#!/usr/bin/env npx ts-node
/**
 * Example client demonstrating the Acton IPC client library.
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
 *   cargo run --example ipc_client_libraries_server
 *
 *   # Then run this client:
 *   npm run example
 *   # or
 *   npx ts-node src/example-client.ts
 */

import {
  ActonIpcClient,
  getDefaultSocketPath,
  socketExists,
  PushNotification,
  ServerError,
  TimeoutError,
  ConnectionError,
} from './index';

async function demoBasicRequest(client: ActonIpcClient): Promise<void> {
  console.log('\n=== Basic Request-Response ===');

  try {
    const response = await client.request(
      'calculator',
      'Add',
      { a: 42, b: 8 },
      5000
    );

    if (response.success) {
      console.log(`  Result: ${JSON.stringify(response.payload)}`);
    } else {
      console.log(`  Error: ${response.error}`);
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
  console.log('\n=== Fire and Forget ===');

  await client.fireAndForget('logger', 'LogEvent', {
    level: 'info',
    message: 'Hello from Node.js client!',
  });

  console.log('  Message sent (no response expected)');
}

async function demoStreaming(client: ActonIpcClient): Promise<void> {
  console.log('\n=== Streaming Response ===');

  try {
    let frameCount = 0;

    for await (const frame of client.stream(
      'search',
      'SearchQuery',
      { query: 'test', limit: 5 },
      10000
    )) {
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
  console.log('\n=== Service Discovery ===');

  try {
    const discovery = await client.discover();

    console.log(`  Protocol: v${discovery.protocol_version?.current ?? '?'}`);
    console.log(`  Capabilities: ${JSON.stringify(discovery.protocol_version?.capabilities ?? {})}`);
    console.log(`  Agents: ${discovery.agents?.map(a => a.name).join(', ') || 'none'}`);
    console.log(`  Message Types: ${discovery.message_types?.join(', ') || 'none'}`);
  } catch (err) {
    console.log(`  Discovery failed: ${err}`);
  }
}

async function demoSubscriptions(client: ActonIpcClient): Promise<void> {
  console.log('\n=== Subscriptions ===');

  const receivedNotifications: PushNotification[] = [];

  client.onPush((notification) => {
    receivedNotifications.push(notification);
    console.log(`  [PUSH] ${notification.message_type}: ${JSON.stringify(notification.payload)}`);
  });

  // Subscribe to message types
  const success = await client.subscribe(['PriceUpdate', 'StatusChange']);
  console.log(`  Subscription result: ${success ? 'success' : 'failed'}`);

  // Wait for notifications
  console.log('  Waiting for notifications (3 seconds)...');
  await new Promise(resolve => setTimeout(resolve, 3000));

  console.log(`  Received ${receivedNotifications.length} notifications`);

  // Unsubscribe
  await client.unsubscribe(['PriceUpdate', 'StatusChange']);
  console.log('  Unsubscribed');
}

async function main(): Promise<void> {
  // Determine socket path
  let socketPath = getDefaultSocketPath('ipc_client_example');

  // Allow override via command line
  if (process.argv[2]) {
    socketPath = process.argv[2];
  }

  console.log('Acton IPC Node.js Client Example');
  console.log('=================================');
  console.log(`Socket path: ${socketPath}`);

  // Check if socket exists
  if (!socketExists(socketPath)) {
    console.log(`\nError: Socket not found at ${socketPath}`);
    console.log('\nMake sure the example server is running:');
    console.log('  cargo run --example ipc_client_libraries_server');
    process.exit(1);
  }

  // Connect and run demos
  const client = new ActonIpcClient(socketPath);

  try {
    await client.connect();
    console.log('\nConnected to server!');

    await demoDiscovery(client);
    await demoBasicRequest(client);
    await demoFireAndForget(client);
    await demoStreaming(client);
    await demoSubscriptions(client);

    console.log('\n=== All demos complete ===');
  } catch (err) {
    if (err instanceof ConnectionError) {
      console.log(`\nConnection error: ${err.message}`);
      process.exit(1);
    }
    throw err;
  } finally {
    await client.close();
  }
}

main().catch(console.error);
