#!/usr/bin/env python3
"""Check every link in the docs-site static export (out/) and report dead ones.

Usage:
    scripts/check-links.py [--out-dir out] [--skip-external] [--timeout 10]

Checks, per HTML file in the export:
  - internal hrefs/srcs resolve to a file in the export
    (Next.js static-export rules: /docs/foo -> docs/foo.html or docs/foo/index.html)
  - with --base-path, root-absolute links must carry the base path — a link like
    /docs/foo escapes a site served under /acton-reactive/ and 404s in production,
    so build the export with the base path applied (GITHUB_ACTIONS=1 pnpm build)
    and pass --base-path /acton-reactive to catch those
  - fragment links (#section, /page#section) point at an existing id in the target page
  - external http(s) links respond with a non-error status (deduplicated across pages)

Exit status is non-zero when any dead link is found, so it can run as a CI gate.
"""

import argparse
import re
import ssl
import sys
import urllib.error
import urllib.parse
import urllib.request
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor, as_completed
from html.parser import HTMLParser
from pathlib import Path

USER_AGENT = (
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) "
    "Chrome/126.0 Safari/537.36 acton-docs-linkcheck"
)
SKIP_SCHEMES = ("mailto:", "tel:", "javascript:", "data:")
# Hosts that block anonymous or scripted requests; a 403/429 from them is not a dead link.
BOT_HOSTILE_STATUSES = {403, 429, 503, 999}


class LinkExtractor(HTMLParser):
    def __init__(self):
        super().__init__()
        self.links = []  # (url, tag)
        self.ids = set()

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if "id" in attrs:
            self.ids.add(attrs["id"])
        if tag == "a" and attrs.get("name"):
            self.ids.add(attrs["name"])
        url = None
        if tag in ("a", "link") and attrs.get("href"):
            url = attrs["href"]
        elif tag in ("img", "script", "source", "iframe") and attrs.get("src"):
            url = attrs["src"]
        if url:
            self.links.append((url, tag))


def parse_html(path: Path) -> LinkExtractor:
    parser = LinkExtractor()
    parser.feed(path.read_text(encoding="utf-8", errors="replace"))
    return parser


def resolve_internal(out_dir: Path, page: Path, target: str, base_path: str) -> Path | None:
    """Return the export file a link points at, or None if nothing matches."""
    if target.startswith("/"):
        if base_path:
            if target != base_path and not target.startswith(base_path + "/"):
                return None  # escapes the base path -> 404 in production
            target = target[len(base_path):] or "/"
        base = out_dir / target.lstrip("/")
    else:
        base = (page.parent / target).resolve()
    candidates = [base]
    if not base.suffix:
        stripped = Path(str(base).rstrip("/"))
        candidates += [stripped.with_suffix(".html"), stripped / "index.html"]
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    return None


def check_external(url: str, timeout: float) -> tuple[str, str | None]:
    """Return (url, problem) where problem is None when the link is alive."""
    ctx = ssl.create_default_context()
    for method in ("HEAD", "GET"):
        req = urllib.request.Request(url, method=method, headers={"User-Agent": USER_AGENT})
        try:
            with urllib.request.urlopen(req, timeout=timeout, context=ctx):
                return url, None
        except urllib.error.HTTPError as err:
            if method == "HEAD":
                continue  # some servers reject HEAD; retry with GET
            if err.code in BOT_HOSTILE_STATUSES:
                return url, f"WARN: HTTP {err.code} (likely bot-blocking, verify manually)"
            return url, f"HTTP {err.code}"
        except Exception as err:  # URLError, timeout, ConnectionReset, ...
            if method == "HEAD":
                continue
            return url, f"{type(err).__name__}: {err}"
    return url, "unreachable"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--out-dir", default=None, help="static export dir (default: <site>/out)")
    ap.add_argument("--base-path", default="",
                    help="production base path, e.g. /acton-reactive; root-absolute "
                         "links that do not start with it are reported dead")
    ap.add_argument("--skip-external", action="store_true", help="only check internal links")
    ap.add_argument("--timeout", type=float, default=10.0, help="external request timeout (s)")
    ap.add_argument("--workers", type=int, default=16, help="parallel external requests")
    args = ap.parse_args()

    site_dir = Path(__file__).resolve().parent.parent
    out_dir = Path(args.out_dir) if args.out_dir else site_dir / "out"
    if not out_dir.is_dir():
        print(f"error: export dir {out_dir} not found — run the site build first", file=sys.stderr)
        return 2

    pages = sorted(out_dir.rglob("*.html"))
    parsed = {page: parse_html(page) for page in pages}
    ids_by_file = {page: doc.ids for page, doc in parsed.items()}

    dead_internal = []  # (page, link, reason)
    external = defaultdict(set)  # url -> {pages}

    for page, doc in parsed.items():
        rel_page = page.relative_to(out_dir)
        for url, _tag in doc.links:
            if url.startswith(SKIP_SCHEMES):
                continue
            if url.startswith(("http://", "https://", "//")):
                full = "https:" + url if url.startswith("//") else url
                external[full].add(str(rel_page))
                continue
            path_part, _, fragment = url.partition("#")
            if path_part:
                target = resolve_internal(out_dir, page, urllib.parse.unquote(path_part),
                                          args.base_path.rstrip("/"))
                if target is None:
                    reason = "no matching file in export"
                    if args.base_path and path_part.startswith("/") \
                            and not path_part.startswith(args.base_path.rstrip("/")):
                        reason = f"escapes base path {args.base_path} -> 404 in production"
                    dead_internal.append((rel_page, url, reason))
                    continue
            else:
                target = page  # same-page anchor
            if fragment and target.suffix == ".html":
                if fragment not in ids_by_file.get(target, set()):
                    dead_internal.append((rel_page, url, f"missing anchor in {target.relative_to(out_dir)}"))

    print(f"scanned {len(pages)} pages: "
          f"{sum(len(d.links) for d in parsed.values())} links, "
          f"{len(external)} unique external URLs")

    dead_external = []  # (url, problem, pages)
    warn_external = []
    if not args.skip_external:
        with ThreadPoolExecutor(max_workers=args.workers) as pool:
            futures = {pool.submit(check_external, url, args.timeout): url for url in external}
            for future in as_completed(futures):
                url, problem = future.result()
                if problem is None:
                    continue
                bucket = warn_external if problem.startswith("WARN") else dead_external
                bucket.append((url, problem, sorted(external[url])))

    ok = True
    if dead_internal:
        ok = False
        print(f"\nDEAD INTERNAL LINKS ({len(dead_internal)}):")
        for page, url, reason in sorted(dead_internal):
            print(f"  {page}: {url}  [{reason}]")
    if dead_external:
        ok = False
        print(f"\nDEAD EXTERNAL LINKS ({len(dead_external)}):")
        for url, problem, sources in sorted(dead_external):
            print(f"  {url}  [{problem}]")
            for src in sources:
                print(f"      referenced by {src}")
    if warn_external:
        print(f"\nEXTERNAL WARNINGS ({len(warn_external)}) — bot-blocked, verify manually:")
        for url, problem, sources in sorted(warn_external):
            print(f"  {url}  [{problem}]  (from {', '.join(sources)})")
    if ok and not warn_external:
        print("\nall links OK")
    elif ok:
        print("\nno dead links; warnings above need manual eyes")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
