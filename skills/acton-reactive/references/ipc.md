# Cross-process actors (the `ipc` feature)

```toml
acton-reactive = { version = "9", features = ["ipc"] }
# or "ipc-messagepack" for compact binary framing (~30-50% smaller)
```

Actors reachable over a Unix domain socket, so a separate process can send them
messages. The actor code does not change: exposing an actor is a runtime
concern, not a handler concern.

## Server side

```rust
let handle = builder.start().await;
app.ipc_expose("inventory", handle)?;          // Result<(), IpcNameInUse>
let listener = app.start_ipc_listener().await?;
```

`ipc_expose` returns a `Result` in 9.x — the name is a namespace and collisions
are an error, not a last-write-wins. The exposed name is what remote callers
address; it is independent of the actor's `Ern`.

Message types crossing the boundary must be registered so both sides agree on
the wire name:

```rust
registry.register::<Restock>("Restock");
```

`start_ipc_listener_with_config` takes an `IpcConfig` for the socket path,
connection limit (default 1024), and framing.

## Client side

```rust
let client = IpcClient::connect("/run/myapp/ipc.sock").await?;
let response: IpcResponse = client.request(envelope).await?;
```

| Method | Use for |
|---|---|
| `connect` / `connect_with_config` | opening the connection |
| `request` / `request_with_timeout` | one request, one response |
| `request_stream` / `request_stream_with_timeout` | one request, many responses |

`request_with_timeout` bounds a remote exchange the way `ask_with_timeout`
bounds a local one. Bound them: a remote peer that never answers is a much more
likely failure than a local actor that never answers.

## Typed remote requests

`RemoteRequest` is the cross-process analogue of `Request`, so a remote call can
be typed the same way a local `ask` is rather than passing envelopes around by
hand. Use it wherever the response type is known at compile time.

## Push to the client

Remote peers can subscribe to broker broadcasts, so the server can push without
the client polling. This is the right shape for status feeds and progress
updates; polling over IPC is the same anti-pattern as polling in-process.

## Configuration

`IpcConfig` is populated from an `ipc.toml` searched in two locations (project,
then XDG config). Keep the socket path outside `/tmp` for anything that
outlives a session.

`PeerCredentials` gives you the connecting process's uid/gid/pid. A Unix socket
grants access to anyone who can open the path, so if the actor does anything
privileged, check credentials rather than trusting filesystem permissions
alone.

## Design note

A second process is acton's unit of isolation, not parallelism. Parallelism is
already free: every actor is a task on the work-stealing runtime, so more cores
never require another process (contrast actix, where parallelism means placing
actors on new arbiter threads). Reach for a peer instance when the work needs a
separate failure or resource domain — crashy FFI, memory-hungry native code, a
different lifecycle — and accept serialization at the boundary as the price of
the bulkhead. The peer is a full acton runtime: its actors get supervision,
restart policies, and panic containment, and a supervised restart rebinds the
actor's exposed IPC names so remote callers keep addressing a valid handle.

The seam is process death. IPC does not make remote actors equivalent to local
ones: a remote send can fail because the peer is gone, and the failure surfaces
as `IpcError` rather than as a supervision event — the supervisor knows nothing
about a process it did not start. The process itself needs an external
supervisor (systemd or similar), and on the caller side, model the connection
as state owned by an actor that can observe it dropping and react, rather than
assuming a `RemoteActorRef` stays valid.
