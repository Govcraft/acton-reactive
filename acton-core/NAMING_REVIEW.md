# Acton Core Naming Convention Review Checklist (Revised)

**Target Audience:** Developers potentially new to Rust, coming from other languages. May be unfamiliar with or intimidated by traditional "actor framework" jargon and concepts.
**Goal:** Ensure naming is simple, intuitive, non-intimidating, and hides unnecessary complexity, providing an easy-to-use API while leveraging actor model benefits internally.

---

## `src/actor/agent_config.rs`

*   **[x] `AgentConfig::ern` (Field)**
    *   **Observation:** Uses the acronym "ERN" (Entity Resource Name). This is framework-specific jargon and potentially confusing. The `AgentHandle::id` method also returns this.
    *   **Suggestion:** **Strongly recommend renaming `ern` to `id`**. This is a common and understandable term for a unique identifier. Avoid acronyms in the public API. Ensure documentation clearly explains that this `id` can be hierarchical if needed, but focus on its primary role as an identifier.

## `src/actor/managed_agent.rs`

*   **[x] `ManagedAgent` (Struct Name)**
    *   **Observation:** The term "Managed" might imply complexity or framework control that could be slightly intimidating.
    *   **Suggestion:** Consider if just `Agent` would suffice, or perhaps `AgentController` or `AgentCore` if distinguishing from the user's state struct is necessary. However, `ManagedAgent` might be acceptable if documentation clearly explains it simply means "the framework helps manage this agent's lifecycle". Evaluate if the prefix adds necessary clarity or just jargon. (Leaning towards keeping if well-explained, but worth considering simplification).
*   **[x] `ManagedAgent::reactors` (Field)**
    *   **Observation:** "Reactors" is framework-specific jargon tied to the "Reactive" name but not intuitive for message handling.
    *   **Suggestion:** **Strongly recommend renaming `reactors` to `message_handlers`**. This is direct, descriptive, and uses common terminology.

## `src/common/agent_runtime.rs`

*   **[x] `AgentRuntime` Methods: `new_agent`, `create_actor`, `spawn_actor`**
    *   **Observation:** Multiple methods with mixed "agent" and "actor" terminology appear to exist for creating top-level agents based on the architecture overview. "Actor" terminology should be avoided in the user-facing API for approachability.
    *   **Suggestion:** **Strongly recommend unifying the naming and sticking strictly to "agent" based on the actual code implementation.**
        *   Use `new_agent()` for configuring an agent (returning the `ManagedAgent<Idle>` or similar builder).
        *   Use `start_agent()` (or similar verb like `launch_agent`) for configuring *and* starting the agent (returning the `AgentHandle`).
        *   Remove or clearly differentiate any methods using `create_actor` or `spawn_actor` if they exist in the code, ensuring the public API consistently uses "agent".

## `src/common/types.rs`

*   **[x] `Outbox` (Type Alias)**
    *   **Observation:** This is common actor terminology but might be jargon to newcomers. It refers to the sending end of the agent's message channel.
    *   **Suggestion:** Consider renaming the type alias to something more descriptive like `MessageSender` or `AgentSender`. This clarifies the purpose without relying on actor-specific terms.
*   **[x] `BrokerRef` / `ParentRef` (Type Aliases)**
    *   **Observation:** These wrap `AgentHandle` for semantic roles.
    *   **Suggestion:** Keep these aliases as they add clarity. Ensure documentation explains the *roles* in simple terms:
        *   `ParentRef`: A handle to the agent that created this one.
        *   `BrokerRef`: A handle to a special agent responsible for broadcasting messages to many other agents (like a central notice board). Avoid overly technical explanations of the pub/sub pattern initially.

## Traits (`src/traits/`)

*   **[x] `Actor` (Trait Name)**
    *   **Observation:** This is the most prominent use of potentially intimidating actor jargon in the public API.
    *   **Suggestion:** **Strongly recommend renaming the `Actor` trait.** Options:
        *   `AgentHandleInterface`: Explicitly links it to the `AgentHandle`.
        *   `AgentInterface`: More general.
        *   `Agent`: If the name isn't already taken by the user's state struct type parameter.
        *   If renaming is too disruptive, documentation *must* clearly state: "This trait defines the standard operations for interacting with an agent via its `AgentHandle`. While named `Actor` due to underlying concepts, you primarily work with Agents in this framework." Emphasize "Agent" elsewhere.
*   **[x] `Broker` / `Subscriber` / `Subscribable` (Traits)**
    *   **Observation:** These relate to the publish/subscribe mechanism. "Broker" and "Subscriber" are relatively common terms.
    *   **Suggestion:** These names are likely acceptable, but ensure the *concept* is explained simply in documentation, focusing on the *what* (sending messages widely, receiving broadcast messages) rather than the *how* (complex pub/sub patterns) initially.

## General Observations

*   **Agent vs. Actor:** Reinforce the decision to **consistently use "Agent"** in all user-facing APIs, documentation, and examples. Minimize or eliminate the term "Actor" where possible, especially in core types like traits and creation methods.
*   **Simplicity over Precision (in Naming):** When choosing names, lean towards terms that are immediately understandable to someone from a different programming background, even if a slightly more precise actor-model term exists. The goal is to lower the barrier to entry.

---

This revised checklist focuses on making the framework feel welcoming and straightforward for developers new to Rust and actor concepts, based on the code as the source of truth.