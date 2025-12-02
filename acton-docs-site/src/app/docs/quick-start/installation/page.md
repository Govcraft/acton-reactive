---
title: Installation
description: Add Acton Reactive to your project in under 30 seconds.
---

Getting started with Acton Reactive takes about 30 seconds.

## Prerequisites

You need:

- **Rust 1.70 or later** (Rust 2021 edition)
- **Cargo** (comes with Rust)

If you don't have Rust yet, visit [rustup.rs](https://rustup.rs). The default options work fine.

{% callout title="New to Rust?" %}
Acton Reactive is designed to make concurrent programming approachable. We explain Rust concepts as we go.
{% /callout %}

## Add Acton Reactive to Your Project

Starting a new project:

```shell
cargo new my-first-actor
cd my-first-actor
cargo add acton-reactive
```

Adding to an existing project:

```shell
cargo add acton-reactive
```

That's it. Cargo handles everything else.

## Verify It Works

Replace the contents of `src/main.rs` with:

```rust
use acton_reactive::prelude::*;

#[acton_main]
async fn main() {
    println!("Acton Reactive is ready!");
}
```

Run it:

```shell
cargo run
```

You should see:

```
Acton Reactive is ready!
```

{% callout type="note" title="First Build Takes a Moment" %}
The first compile downloads and builds dependencies. Subsequent builds are much faster.
{% /callout %}

## What Just Happened?

That example introduced two things:

1. **`use acton_reactive::prelude::*`** — Imports everything you need from Acton Reactive. The prelude pattern saves you from importing dozens of individual items.

2. **`#[acton_main]`** — Sets up the async runtime that actors need. Use this instead of the standard `fn main()` for Acton Reactive applications.

These concepts will make more sense as you build your first actor.

---

## Next Step

You're set up. Let's build something.

[Your First Actor](/docs/quick-start/your-first-actor) — Build a working actor in about 5 minutes.
