# Configuration: what goes in a file, and where

## The dividing line

The question to ask about any setting is **"would an operator change this
without recompiling?"**

| Belongs in `config.toml` | Belongs in code |
|---|---|
| Timeouts, inbox capacity, handler high-water mark | Restart policy, supervision strategy, escalation |
| Log/cache/data directories | Restart limiter thresholds |
| Tracing verbosity, metrics on/off | Which actors exist and how they are wired |
| Socket paths and connection limits (`ipc.toml`) | Per-actor capacity where one actor has known burst behaviour |

The reasoning: settings in the file are **tuning** — a wrong value makes the
system slower or noisier, and the fix is an edit and a restart. Settings in
code are **correctness** — a wrong `RestartPolicy` is a bug, and it should go
through review and tests like any other bug. Putting supervision strategy in a
config file means a production edit can silently change your failure semantics.

Per-actor overrides in code are not a violation of this. `with_inbox_capacity`
on the one actor that receives bursts is a design decision about that actor;
the global default in the file is the operator's floor.

## Where the file lives

`ActonConfig::load()` runs once, lazily, on first use. It searches
XDG-compliant locations for `acton/config.toml`:

1. `$XDG_CONFIG_HOME/acton/config.toml`
2. `~/.config/acton/config.toml` (Linux fallback)
3. `~/Library/Application Support/acton/config.toml` (macOS)
4. `%APPDATA%/acton/config.toml` (Windows)

**No file is not an error** — you get defaults. A file that exists but is
malformed logs an error and *also* falls back to defaults, so a typo does not
crash the program, it silently un-tunes it. If a setting seems to be ignored,
check the startup logs for a parse error before assuming the key is wrong.

## Different settings per environment

There is no "environment" concept and no `--config` flag. The loader only ever
looks for `acton/config.toml` under the XDG config directories, so the way to
give staging and production different values is to **point `XDG_CONFIG_HOME` at
a different directory per deployment**:

```ini
# /etc/systemd/system/ingest.service.d/override.conf
[Service]
Environment=XDG_CONFIG_HOME=/etc/ingest-service/prod
```

with the file at `/etc/ingest-service/prod/acton/config.toml`. Staging points
at its own directory with its own copy. This keeps one binary and one code
path, which is the thing ops actually asked for.

Note that `XDG_CONFIG_HOME` is process-wide, so it also relocates any *other*
XDG-aware config your program reads. That is usually what you want, but it is
worth knowing before you set it.

## Precedence

```
ActorConfig::with_*(..) in code      (per actor, wins)
        ↓
config.toml                          (process-wide default)
        ↓
built-in defaults                    (compiled in)
```

## Every key, with its default

```toml
[timeouts]                       # milliseconds
actor_shutdown = 10000
system_shutdown = 30000
read_only_handler_flush = 10

[limits]
concurrent_handlers_high_water_mark = 100   # concurrent act_on futures
actor_inbox_capacity = 512                  # default mailbox depth
dummy_channel_size = 1

[defaults]
actor_name = "actor"
root_ern = "default"

[tracing]
debug = "debug"
trace = "trace"
info = "info"

[paths]
logs = "~/.local/share/acton/logs"
cache = "~/.cache/acton"
data = "~/.local/share/acton"
config = "~/.config/acton"

[behavior]
enable_tracing = true
enable_metrics = false
```

The two worth understanding rather than copying:

- **`actor_inbox_capacity`** is backpressure. When an actor's mailbox is full,
  `send` waits. Raising it globally hides a slow consumer instead of fixing it;
  raise it for the one actor that genuinely bursts, in code.
- **`concurrent_handlers_high_water_mark`** caps in-flight `act_on` futures.
  This is why `tokio::spawn` in a handler is harmful: the spawned task escapes
  this cap, so the backpressure you configured stops applying.

## You cannot read this config from your own code

`ActonConfig` and the loaded `CONFIG` singleton live in `pub(crate) mod common`
(`lib.rs:56`). They are **not** exported, not in the prelude, and not reachable
from a dependent crate. Do not write `use acton_reactive::common::...` — it
will not compile.

The practical consequences:

- The TOML file is the only lever on these values. The framework reads it; you
  cannot inspect what it read.
- Anything your *own* code needs to branch on — a vendor URL, a batch size,
  a feature switch — needs your own config, loaded your own way, and passed
  into actors at wiring time. Do not expect to piggyback on `acton/config.toml`.
- Because you cannot read it back, a malformed file is doubly quiet: it falls
  back to defaults, and nothing in your program can notice. The startup log is
  the only signal.

Pass configuration into actors as state at construction rather than having
handlers reach for a global. An actor whose limits are fields is testable at
different limits; one that reads a global is not.

## IPC configuration is separate

The `ipc` feature reads `ipc.toml`, not `config.toml`, and searches two tiers:

1. `$XDG_CONFIG_HOME/acton/<app_name>/ipc.toml` — per-application
2. `$XDG_CONFIG_HOME/acton/ipc.toml` — shared by every Acton IPC server

Per-application wins. The shared tier is for things that are genuinely
machine-wide, like a socket directory convention. Put the socket path outside
`/tmp` for anything that should outlive a login session.

## What to tell users of your program

If your program ships with tuned defaults, say so in its README: which file it
reads, which keys it cares about, and what the values mean operationally.
A config system nobody documents is a config system nobody uses, and the
defaults become the only configuration that exists.
