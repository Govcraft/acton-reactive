---
title: Next Steps
description: You know the basics. Here's how to go deeper with Acton Reactive.
---

You've learned the fundamentals of actor-based programming with Acton Reactive. Let's recap and chart a path forward.

## What You've Learned

In just a few pages, you've covered the essentials:

- **Actors** hold state and process messages one at a time
- **Messages** are the only way to communicate with actors
- **Handlers** define how actors respond: `mutate_on` for changes, `act_on` for reads
- **Send** fires a message without waiting
- **Ask** requests data and waits for the reply
- **Reply** signals completion: `Reply::ready()` or `Reply::with(value)`

These building blocks are enough to build real applications.

---

## Recommended Path

### Core Concepts (Next)

Deepen your understanding:

- [What Are Actors?](/docs/core-concepts/what-are-actors) - The mental model behind actors
- [Messages & Handlers](/docs/core-concepts/messages-and-handlers) - Deep dive into communication
- [The Actor System](/docs/core-concepts/the-actor-system) - Managing your actors
- [Supervision Basics](/docs/core-concepts/supervision-basics) - Handling failures

### Building Apps

Practical patterns:

- [Parent-Child Actors](/docs/building-apps/parent-child-actors) - Hierarchical organization
- [Request-Response](/docs/building-apps/request-response) - Complex communication
- [Error Handling](/docs/building-apps/error-handling) - Building resilient systems
- [Testing Actors](/docs/building-apps/testing-actors) - Testing strategies

### Advanced Topics

- [IPC](/docs/advanced/ipc) - Cross-process communication
- [Performance](/docs/advanced/performance) - Optimization strategies

---

## Quick Wins to Try

Before diving into more documentation, try extending what you've built:

### Challenge 1: Multiple Actors

Create two counters running simultaneously:

```rust
let counter_a = builder_a.start().await;
let counter_b = builder_b.start().await;

counter_a.send(Increment).await;
counter_b.send(Increment).await;
```

### Challenge 2: Add More Messages

Extend your counter with `Decrement` and `Reset` messages.

### Challenge 3: Calculator

Build a calculator actor with `Add`, `Multiply`, `GetResult` messages.

---

## Common Questions

### "When should I use actors vs regular async code?"

Actors shine when you have:
- State that needs to be modified safely from multiple places
- Long-lived processes that handle events over time
- Systems that benefit from isolation and message-passing

For simple one-shot async operations, regular async/await is fine.

### "Is this production-ready?"

Yes! Acton Reactive is designed for production use. The patterns you've learned scale to complex systems.

---

## Getting Help

- **API Reference** - [docs.rs/acton-reactive](https://docs.rs/acton-reactive)
- **Examples** - [GitHub examples](https://github.com/Govcraft/acton-reactive/tree/main/acton-reactive/examples)

---

## You're Ready

Start with something small. Experiment. That's how expertise develops.

Welcome to actor-based programming.

[Continue to Core Concepts](/docs/core-concepts/what-are-actors)
