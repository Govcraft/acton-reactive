---
title: Installation
description: Add Acton Reactive to your project in under 30 seconds.
---

Getting started with Acton Reactive takes about 30 seconds. Let's get you set up.

## Prerequisites

You'll need:

- **Rust 1.75 or later** - Acton Reactive uses async features that require a recent Rust version
- **Cargo** - Rust's package manager (comes with Rust)

If you don't have Rust yet, visit [rustup.rs](https://rustup.rs) for a quick installation. The default options work great.

{% callout title="New to Rust?" %}
That's perfectly fine! Acton Reactive is designed to make concurrent programming approachable. We'll explain Rust concepts as we go.
{% /callout %}

## Add Acton Reactive to Your Project

If you're starting a new project:

```shell
cargo new my-first-actor
cd my-first-actor
cargo add acton-reactive
```

If you're adding to an existing project:

```shell
cargo add acton-reactive
```

That's it. One command. Cargo handles everything else.

## Verify It Works

Let's make sure everything is set up correctly. Replace the contents of `src/main.rs` with:

```rust
use acton_reactive::prelude::*;

#[acton_main]
async fn main() {
    println!("Acton Reactive is ready!");
}
```

Now run it:

```shell
cargo run
```

You should see:

```
Acton Reactive is ready!
```

{% callout type="note" title="First Build Takes a Moment" %}
The first time you compile, Cargo downloads and builds dependencies. This is normal and only happens once. Subsequent builds are much faster.
{% /callout %}

## What Just Happened?

That small example introduced two things:

1. **`use acton_reactive::prelude::*`** - This brings in everything you need from Acton Reactive. The prelude pattern saves you from importing dozens of individual items.

2. **`#[acton_main]`** - This attribute sets up the async runtime that actors need. You'll use this instead of the standard `fn main()` for Acton Reactive applications.

Don't worry if these concepts aren't fully clear yet. They'll make more sense as you build your first actor.

---

## Next Step

You're all set up. Let's build something!

[Your First Actor](/docs/quick-start/your-first-actor) - Build a working actor in about 5 minutes.
