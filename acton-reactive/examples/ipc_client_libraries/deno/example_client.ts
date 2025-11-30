#!/usr/bin/env -S deno run --allow-read --allow-env --allow-net=unix
/**
 * Example client demonstrating the Acton IPC client library for Deno.
 *
 * This example shows how to:
 * 1. Connect to an acton-reactive IPC server
 * 2. Send request-response messages
 * 3. Use streaming responses
 * 4. Subscribe to push notifications
 * 5. Discover available services
 *
 * Usage:
 *   # First, start the example server (from the Rust side):
 *   cargo run --example ipc_client_libraries_server --features ipc
 *
 *   # Then run this client:
 *   deno run --allow-read --allow-env --allow-net=unix example_client.ts
 *
 *   # Or make it executable and run directly:
 *   chmod +x example_client.ts
 *   ./example_client.ts
 */

import {
  ActonIpcClient,
  ConnectionError,
  getDefaultSocketPath,
  type PushNotification,
  ServerError,
  socketExists,
  TimeoutError,
} from "./acton_ipc.ts";

async function demoBasicRequest(client: ActonIpcClient): Promise<void> {
  console.log("\n=== Basic Request-Response ===");

  try {
    const response = await client.request(
      "calculator",
      "Add",
      { a: 42, b: 8 },
      5000,
    );

    if (response.success) {
      console.log(`  Result: ${JSON.stringify(response.payload)}`);
    } else {
      console.log(`  Error: ${response.error}`);
    }

    // Try multiplication too
    const mulResponse = await client.request(
      "calculator",
      "Multiply",
      { a: 7, b: 6 },
      5000,
    );

    if (mulResponse.success) {
      console.log(`  Multiply Result: ${JSON.stringify(mulResponse.payload)}`);
    }
  } catch (err) {
    if (err instanceof ServerError) {
      console.log(`  Server error: ${err.message} (code: ${err.code})`);
    } else if (err instanceof TimeoutError) {
      console.log(`  Timeout: ${err.message}`);
    } else {
      throw err;
    }
  }
}

async function demoFireAndForget(client: ActonIpcClient): Promise<void> {
  console.log("\n=== Fire and Forget ===");

  await client.fireAndForget("logger", "LogEvent", {
    level: "info",
    message: "Hello from Deno client!",
  });

  console.log("  Message sent (no response expected)");

  // Send a few more log messages
  await client.fireAndForget("logger", "LogEvent", {
    level: "debug",
    message: "Deno runtime version: " + Deno.version.deno,
  });

  await client.fireAndForget("logger", "LogEvent", {
    level: "warn",
    message: "This is a warning from Deno",
  });

  console.log("  Sent 3 log messages total");
}

async function demoStreaming(client: ActonIpcClient): Promise<void> {
  console.log("\n=== Streaming Response ===");

  try {
    let frameCount = 0;

    for await (
      const frame of client.stream(
        "search",
        "SearchQuery",
        { query: "deno", limit: 5 },
        10000,
      )
    ) {
      frameCount++;

      if (frame.error) {
        console.log(`  Stream error: ${frame.error}`);
        break;
      }

      console.log(`  Frame ${frame.sequence}: ${JSON.stringify(frame.payload)}`);

      if (frame.is_final) {
        console.log(`  Stream complete (${frameCount} frames)`);
        break;
      }
    }
  } catch (err) {
    if (err instanceof TimeoutError) {
      console.log(`  Stream timeout: ${err.message}`);
    } else {
      throw err;
    }
  }
}

async function demoDiscovery(client: ActonIpcClient): Promise<void> {
  console.log("\n=== Service Discovery ===");

  try {
    const discovery = await client.discover();

    console.log(
      `  Protocol: v${discovery.protocol_version?.current ?? "?"}`,
    );
    console.log(
      `  Capabilities: ${JSON.stringify(discovery.protocol_version?.capabilities ?? {})}`,
    );
    console.log(
      `  Agents: ${discovery.agents?.map((a) => a.name).join(", ") || "none"}`,
    );
    console.log(
      `  Message Types: ${discovery.message_types?.join(", ") || "none"}`,
    );
  } catch (err) {
    console.log(`  Discovery failed: ${err}`);
  }
}

async function demoSubscriptions(client: ActonIpcClient): Promise<void> {
  console.log("\n=== Subscriptions ===");

  const receivedNotifications: PushNotification[] = [];

  client.onPush((notification) => {
    receivedNotifications.push(notification);
    console.log(
      `  [PUSH] ${notification.message_type}: ${JSON.stringify(notification.payload)}`,
    );
  });

  // Subscribe to message types
  const success = await client.subscribe(["PriceUpdate", "StatusChange"]);
  console.log(`  Subscription result: ${success ? "success" : "failed"}`);

  // Wait for notifications (the server sends PriceUpdate every 5 seconds)
  console.log("  Waiting for notifications (8 seconds)...");
  await delay(8000);

  console.log(`  Received ${receivedNotifications.length} notifications`);

  // Unsubscribe
  await client.unsubscribe(["PriceUpdate", "StatusChange"]);
  console.log("  Unsubscribed");
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function main(): Promise<void> {
  // Determine socket path
  let socketPath = getDefaultSocketPath("ipc_client_example");

  // Allow override via command line
  if (Deno.args[0]) {
    socketPath = Deno.args[0];
  }

  console.log("Acton IPC Deno Client Example");
  console.log("==============================");
  console.log(`Deno version: ${Deno.version.deno}`);
  console.log(`Socket path: ${socketPath}`);

  // Check if socket exists
  if (!(await socketExists(socketPath))) {
    console.log(`\nError: Socket not found at ${socketPath}`);
    console.log("\nMake sure the example server is running:");
    console.log("  cargo run --example ipc_client_libraries_server --features ipc");
    Deno.exit(1);
  }

  // Connect and run demos
  const client = new ActonIpcClient(socketPath);

  try {
    await client.connect();
    console.log("\nConnected to server!");

    await demoDiscovery(client);
    await demoBasicRequest(client);
    await demoFireAndForget(client);
    await demoStreaming(client);
    await demoSubscriptions(client);

    console.log("\n=== All demos complete ===");
  } catch (err) {
    if (err instanceof ConnectionError) {
      console.log(`\nConnection error: ${err.message}`);
      Deno.exit(1);
    }
    throw err;
  } finally {
    client.close();
  }
}

// Run main
main().catch((err) => {
  console.error("Fatal error:", err);
  Deno.exit(1);
});
