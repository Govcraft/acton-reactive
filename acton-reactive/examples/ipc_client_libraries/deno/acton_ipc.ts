/**
 * Acton IPC Client Library for Deno
 *
 * A native Deno client for communicating with acton-reactive IPC servers.
 * Uses Deno's built-in Unix socket support and native TypeScript.
 *
 * @example
 * ```typescript
 * import { ActonIpcClient, getDefaultSocketPath } from "./acton_ipc.ts";
 *
 * const socketPath = getDefaultSocketPath("my_app");
 * const client = new ActonIpcClient(socketPath);
 *
 * await client.connect();
 * const response = await client.request("calculator", "Add", { a: 5, b: 3 });
 * console.log(response.payload);
 * await client.close();
 * ```
 *
 * @module
 */

// ============================================================================
// Protocol Constants
// ============================================================================

const PROTOCOL_VERSION = 0x02;
const MAX_FRAME_SIZE = 16 * 1024 * 1024; // 16 MiB

// Message types
const MSG_TYPE_REQUEST = 0x01;
const MSG_TYPE_RESPONSE = 0x02;
const MSG_TYPE_ERROR = 0x03;
const MSG_TYPE_HEARTBEAT = 0x04;
const MSG_TYPE_PUSH = 0x05;
const MSG_TYPE_SUBSCRIBE = 0x06;
const MSG_TYPE_STREAM = 0x09;
const MSG_TYPE_DISCOVER = 0x08;
const MSG_TYPE_UNSUBSCRIBE = 0x07;

// Serialization formats
const FORMAT_JSON = 0x01;
const FORMAT_MSGPACK = 0x02;

// ============================================================================
// Error Classes
// ============================================================================

/** Error thrown when connection fails */
export class ConnectionError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ConnectionError";
  }
}

/** Error thrown when request times out */
export class TimeoutError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "TimeoutError";
  }
}

/** Error thrown when server returns an error */
export class ServerError extends Error {
  code: string;

  constructor(message: string, code: string) {
    super(message);
    this.name = "ServerError";
    this.code = code;
  }
}

/** Error thrown for protocol violations */
export class ProtocolError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ProtocolError";
  }
}

// ============================================================================
// Types
// ============================================================================

/** IPC message envelope */
export interface IpcEnvelope {
  correlation_id: string;
  target: string;
  message_type: string;
  payload: unknown;
  expects_reply?: boolean;
  expects_stream?: boolean;
  response_timeout_ms?: number;
}

/** IPC response from server */
export interface IpcResponse {
  correlation_id: string;
  success: boolean;
  error?: string;
  error_code?: string;
  payload?: unknown;
}

/** Stream frame from server */
export interface StreamFrame {
  correlation_id: string;
  sequence: number;
  is_final: boolean;
  payload?: unknown;
  error?: string;
}

/** Push notification from server */
export interface PushNotification {
  message_type: string;
  payload: unknown;
  timestamp_ms: number;
}

/** Actor info from discovery */
export interface ActorInfo {
  name: string;
  ern: string;
}

/** Protocol version info */
export interface ProtocolVersionInfo {
  current: number;
  min_supported: number;
  max_supported: number;
  capabilities: Record<string, boolean>;
}

/** Discovery response */
export interface DiscoveryResponse {
  actors: ActorInfo[];
  message_types: string[];
  protocol_version?: ProtocolVersionInfo;
}

/** Subscription request */
interface SubscribeRequest {
  correlation_id: string;
  message_types: string[];
}

/** Client options */
export interface ClientOptions {
  /** Use MessagePack instead of JSON (not yet supported in Deno) */
  useMessagePack?: boolean;
  /** Default timeout for requests in milliseconds */
  defaultTimeout?: number;
}

// ============================================================================
// Utility Functions
// ============================================================================

/**
 * Get the default socket path for an application.
 *
 * Follows XDG Base Directory specification:
 * - Uses $XDG_RUNTIME_DIR/acton/<app_name>/ipc.sock if available
 * - Falls back to /tmp/acton/<app_name>/ipc.sock
 */
export function getDefaultSocketPath(appName: string): string {
  const xdgRuntimeDir = Deno.env.get("XDG_RUNTIME_DIR");
  const baseDir = xdgRuntimeDir || "/tmp";
  return `${baseDir}/acton/${appName}/ipc.sock`;
}

/**
 * Check if a socket file exists.
 */
export async function socketExists(path: string): Promise<boolean> {
  try {
    const stat = await Deno.stat(path);
    return stat.isFile || stat.isSocket !== undefined;
  } catch {
    return false;
  }
}

/**
 * Generate a unique correlation ID (MTI-compatible format).
 */
function generateCorrelationId(prefix = "req"): string {
  const timestamp = Date.now().toString(16).padStart(12, "0");
  const random = Array.from(crypto.getRandomValues(new Uint8Array(8)))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
  return `${prefix}_${timestamp}${random}`;
}

// ============================================================================
// Frame Encoding/Decoding
// ============================================================================

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

function encodeFrame(
  msgType: number,
  payload: unknown,
  format: number = FORMAT_JSON,
): Uint8Array {
  let payloadBytes: Uint8Array;

  if (format === FORMAT_JSON) {
    const jsonStr = JSON.stringify(payload);
    payloadBytes = textEncoder.encode(jsonStr);
  } else {
    throw new Error("MessagePack not yet supported in Deno client");
  }

  // Frame structure: [length:4][version:1][msgType:1][format:1][payload:N]
  // The length field contains ONLY the payload size (not header bytes)
  const payloadLen = payloadBytes.length;
  const frame = new Uint8Array(7 + payloadLen); // 4 + 3 + payload
  const view = new DataView(frame.buffer);

  // Header
  view.setUint32(0, payloadLen, false); // big-endian, payload size only
  frame[4] = PROTOCOL_VERSION;
  frame[5] = msgType;
  frame[6] = format;

  // Payload
  frame.set(payloadBytes, 7);

  return frame;
}

interface DecodedFrame {
  msgType: number;
  format: number;
  payload: unknown;
}

function decodePayload(bytes: Uint8Array, format: number): unknown {
  if (format === FORMAT_JSON) {
    return JSON.parse(textDecoder.decode(bytes));
  } else if (format === FORMAT_MSGPACK) {
    throw new Error("MessagePack not yet supported in Deno client");
  } else {
    throw new ProtocolError(`Unknown format: ${format}`);
  }
}

// ============================================================================
// ActonIpcClient
// ============================================================================

type PushHandler = (notification: PushNotification) => void;

/**
 * Async IPC client for communicating with acton-reactive servers.
 */
export class ActonIpcClient {
  private socketPath: string;
  private conn: Deno.UnixConn | null = null;
  private options: ClientOptions;
  private pendingRequests: Map<
    string,
    {
      resolve: (value: IpcResponse) => void;
      reject: (error: Error) => void;
      timer?: number;
    }
  > = new Map();
  private streamHandlers: Map<
    string,
    {
      frames: StreamFrame[];
      resolve: (frames: StreamFrame[]) => void;
      reject: (error: Error) => void;
      timer?: number;
    }
  > = new Map();
  private pushHandlers: PushHandler[] = [];
  private readLoopRunning = false;
  private readBuffer: Uint8Array = new Uint8Array(0);

  /**
   * Create a new IPC client.
   *
   * @param socketPath - Path to the Unix domain socket
   * @param options - Client options
   */
  constructor(socketPath: string, options: ClientOptions = {}) {
    this.socketPath = socketPath;
    this.options = {
      useMessagePack: false,
      defaultTimeout: 30000,
      ...options,
    };

    if (this.options.useMessagePack) {
      console.warn(
        "MessagePack is not yet supported in Deno client, falling back to JSON",
      );
      this.options.useMessagePack = false;
    }
  }

  /**
   * Connect to the IPC server.
   */
  async connect(): Promise<void> {
    if (this.conn) {
      return;
    }

    try {
      this.conn = await Deno.connect({
        path: this.socketPath,
        transport: "unix",
      });
      this.startReadLoop();
    } catch (err) {
      throw new ConnectionError(
        `Failed to connect to ${this.socketPath}: ${err}`,
      );
    }
  }

  /**
   * Close the connection.
   */
  close(): void {
    this.readLoopRunning = false;
    if (this.conn) {
      try {
        this.conn.close();
      } catch {
        // Ignore close errors
      }
      this.conn = null;
    }

    // Reject all pending requests
    for (const [, pending] of this.pendingRequests) {
      if (pending.timer) clearTimeout(pending.timer);
      pending.reject(new ConnectionError("Connection closed"));
    }
    this.pendingRequests.clear();

    for (const [, stream] of this.streamHandlers) {
      if (stream.timer) clearTimeout(stream.timer);
      stream.reject(new ConnectionError("Connection closed"));
    }
    this.streamHandlers.clear();
  }

  /**
   * Check if connected.
   */
  get isConnected(): boolean {
    return this.conn !== null;
  }

  /**
   * Send a request and wait for a response.
   */
  request(
    target: string,
    messageType: string,
    payload: unknown,
    timeoutMs?: number,
  ): Promise<IpcResponse> {
    if (!this.conn) {
      throw new ConnectionError("Not connected");
    }

    const correlationId = generateCorrelationId();
    const timeout = timeoutMs ?? this.options.defaultTimeout ?? 30000;

    const envelope: IpcEnvelope = {
      correlation_id: correlationId,
      target,
      message_type: messageType,
      payload,
      expects_reply: true,
      response_timeout_ms: timeout,
    };

    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pendingRequests.delete(correlationId);
        reject(new TimeoutError(`Request timed out after ${timeout}ms`));
      }, timeout);

      this.pendingRequests.set(correlationId, { resolve, reject, timer });

      const frame = encodeFrame(MSG_TYPE_REQUEST, envelope);
      this.conn!.write(frame).catch((err) => {
        this.pendingRequests.delete(correlationId);
        clearTimeout(timer);
        reject(new ConnectionError(`Failed to send request: ${err}`));
      });
    });
  }

  /**
   * Send a fire-and-forget message (no response expected).
   */
  async fireAndForget(
    target: string,
    messageType: string,
    payload: unknown,
  ): Promise<void> {
    if (!this.conn) {
      throw new ConnectionError("Not connected");
    }

    const envelope: IpcEnvelope = {
      correlation_id: generateCorrelationId("faf"),
      target,
      message_type: messageType,
      payload,
      expects_reply: false,
    };

    const frame = encodeFrame(MSG_TYPE_REQUEST, envelope);
    await this.conn.write(frame);
  }

  /**
   * Send a streaming request and iterate over response frames.
   */
  async *stream(
    target: string,
    messageType: string,
    payload: unknown,
    timeoutMs?: number,
  ): AsyncGenerator<StreamFrame> {
    if (!this.conn) {
      throw new ConnectionError("Not connected");
    }

    const correlationId = generateCorrelationId("stream");
    const timeout = timeoutMs ?? this.options.defaultTimeout ?? 30000;

    const envelope: IpcEnvelope = {
      correlation_id: correlationId,
      target,
      message_type: messageType,
      payload,
      expects_reply: true,
      expects_stream: true,
      response_timeout_ms: timeout,
    };

    // Set up stream collection
    const frames: StreamFrame[] = [];
    let streamResolve: () => void;
    let streamReject: (err: Error) => void;
    let newFrameResolve: (() => void) | null = null;

    const streamPromise = new Promise<void>((resolve, reject) => {
      streamResolve = resolve;
      streamReject = reject;
    });

    const timer = setTimeout(() => {
      this.streamHandlers.delete(correlationId);
      streamReject(new TimeoutError(`Stream timed out after ${timeout}ms`));
    }, timeout);

    // Custom handler for this stream
    this.streamHandlers.set(correlationId, {
      frames,
      resolve: () => {
        clearTimeout(timer);
        streamResolve();
      },
      reject: (err) => {
        clearTimeout(timer);
        streamReject(err);
      },
      timer,
    });

    // Track frame arrival for yielding
    const originalHandler = this.streamHandlers.get(correlationId)!;
    const frameArrived = () => {
      if (newFrameResolve) {
        newFrameResolve();
        newFrameResolve = null;
      }
    };
    (originalHandler as { onFrame?: () => void }).onFrame = frameArrived;

    // Send the request
    const frame = encodeFrame(MSG_TYPE_REQUEST, envelope);
    await this.conn.write(frame);

    // Yield frames as they arrive
    let yieldedCount = 0;
    let done = false;

    while (!done) {
      // Wait for new frame or completion
      while (yieldedCount >= frames.length && !done) {
        await Promise.race([
          new Promise<void>((resolve) => {
            newFrameResolve = resolve;
          }),
          streamPromise.then(() => {
            done = true;
          }),
        ]);
      }

      // Yield any new frames
      while (yieldedCount < frames.length) {
        const f = frames[yieldedCount++];
        yield f;
        if (f.is_final) {
          done = true;
          break;
        }
      }
    }

    this.streamHandlers.delete(correlationId);
  }

  /**
   * Subscribe to push notification types.
   */
  subscribe(messageTypes: string[]): Promise<boolean> {
    if (!this.conn) {
      throw new ConnectionError("Not connected");
    }

    const correlationId = generateCorrelationId("sub");

    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pendingRequests.delete(correlationId);
        resolve(false); // Timeout, assume failed
      }, 5000);

      this.pendingRequests.set(correlationId, {
        resolve: (response: IpcResponse) => {
          resolve(response.success);
        },
        reject: (err: Error) => {
          reject(err);
        },
        timer,
      });

      const request: SubscribeRequest = {
        correlation_id: correlationId,
        message_types: messageTypes,
      };
      const frame = encodeFrame(MSG_TYPE_SUBSCRIBE, request);
      this.conn!.write(frame).catch(reject);
    });
  }

  /**
   * Unsubscribe from push notification types.
   */
  unsubscribe(messageTypes: string[]): Promise<boolean> {
    if (!this.conn) {
      throw new ConnectionError("Not connected");
    }

    const correlationId = generateCorrelationId("unsub");

    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pendingRequests.delete(correlationId);
        resolve(false); // Timeout, assume failed
      }, 5000);

      this.pendingRequests.set(correlationId, {
        resolve: (response: IpcResponse) => {
          resolve(response.success);
        },
        reject: (err: Error) => {
          reject(err);
        },
        timer,
      });

      const request: SubscribeRequest = {
        correlation_id: correlationId,
        message_types: messageTypes,
      };
      const frame = encodeFrame(MSG_TYPE_UNSUBSCRIBE, request);
      this.conn!.write(frame).catch(reject);
    });
  }

  /**
   * Register a handler for push notifications.
   */
  onPush(handler: PushHandler): void {
    this.pushHandlers.push(handler);
  }

  /**
   * Remove a push notification handler.
   */
  offPush(handler: PushHandler): void {
    const idx = this.pushHandlers.indexOf(handler);
    if (idx >= 0) {
      this.pushHandlers.splice(idx, 1);
    }
  }

  /**
   * Discover available actors and message types.
   */
  discover(): Promise<DiscoveryResponse> {
    if (!this.conn) {
      throw new ConnectionError("Not connected");
    }

    const correlationId = generateCorrelationId("disc");

    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pendingRequests.delete(correlationId);
        reject(new TimeoutError("Discovery timed out"));
      }, 5000);

      this.pendingRequests.set(correlationId, {
        resolve: (response) => {
          if (response.success) {
            // Discovery responses have actors/message_types at top level
            // (not wrapped in payload), so cast the response directly
            const discResponse = response as unknown as {
              actors?: ActorInfo[];
              message_types?: string[];
              protocol_version?: ProtocolVersionInfo;
            };
            resolve({
              actors: discResponse.actors || [],
              message_types: discResponse.message_types || [],
              protocol_version: discResponse.protocol_version,
            });
          } else {
            reject(
              new ServerError(
                response.error || "Discovery failed",
                response.error_code || "UNKNOWN",
              ),
            );
          }
        },
        reject,
        timer,
      });

      const frame = encodeFrame(MSG_TYPE_DISCOVER, {
        correlation_id: correlationId,
        include_actors: true,
        include_message_types: true,
      });
      this.conn!.write(frame).catch((err) => {
        this.pendingRequests.delete(correlationId);
        clearTimeout(timer);
        reject(new ConnectionError(`Failed to send discover: ${err}`));
      });
    });
  }

  /**
   * Send a heartbeat/ping.
   */
  async ping(): Promise<void> {
    if (!this.conn) {
      throw new ConnectionError("Not connected");
    }

    const frame = encodeFrame(MSG_TYPE_HEARTBEAT, {});
    await this.conn.write(frame);
  }

  // -------------------------------------------------------------------------
  // Private Methods
  // -------------------------------------------------------------------------

  private async startReadLoop(): Promise<void> {
    if (this.readLoopRunning || !this.conn) return;
    this.readLoopRunning = true;

    const buffer = new Uint8Array(65536);

    try {
      while (this.readLoopRunning && this.conn) {
        const n = await this.conn.read(buffer);
        if (n === null) {
          // Connection closed
          break;
        }

        // Append to read buffer
        const newBuffer = new Uint8Array(this.readBuffer.length + n);
        newBuffer.set(this.readBuffer);
        newBuffer.set(buffer.subarray(0, n), this.readBuffer.length);
        this.readBuffer = newBuffer;

        // Process complete frames
        this.processFrames();
      }
    } catch (err) {
      if (this.readLoopRunning) {
        console.error("Read loop error:", err);
      }
    } finally {
      this.readLoopRunning = false;
    }
  }

  private processFrames(): void {
    // Frame structure: [length:4][version:1][msgType:1][format:1][payload:length]
    // The length field contains ONLY the payload size (not header bytes)
    const HEADER_SIZE = 7; // 4 (length) + 1 (version) + 1 (msgType) + 1 (format)

    while (this.readBuffer.length >= HEADER_SIZE) {
      // Read frame length (payload size only)
      const lengthBytes = new Uint8Array(4);
      lengthBytes.set(this.readBuffer.subarray(0, 4));
      const view = new DataView(lengthBytes.buffer);
      const payloadLen = view.getUint32(0, false);

      if (payloadLen > MAX_FRAME_SIZE) {
        throw new ProtocolError(`Frame too large: ${payloadLen}`);
      }

      const totalFrameLen = HEADER_SIZE + payloadLen;
      if (this.readBuffer.length < totalFrameLen) {
        break; // Need more data
      }

      // Extract header fields (bytes 4, 5, 6)
      const version = this.readBuffer[4];
      const msgType = this.readBuffer[5];
      const format = this.readBuffer[6];

      // Extract payload (bytes 7 onwards)
      const payloadBytes = new Uint8Array(payloadLen);
      payloadBytes.set(this.readBuffer.subarray(HEADER_SIZE, totalFrameLen));

      // Advance the read buffer
      const remaining = this.readBuffer.length - totalFrameLen;
      if (remaining > 0) {
        const newBuf = new Uint8Array(remaining);
        newBuf.set(this.readBuffer.subarray(totalFrameLen));
        this.readBuffer = newBuf;
      } else {
        this.readBuffer = new Uint8Array(0);
      }

      if (version !== PROTOCOL_VERSION && version !== 0x01) {
        console.warn(`Unknown protocol version: ${version}`);
      }

      try {
        const payload = decodePayload(payloadBytes, format);
        this.handleFrame(msgType, payload);
      } catch (err) {
        console.error("Failed to decode frame:", err);
      }
    }
  }

  private handleFrame(msgType: number, payload: unknown): void {
    switch (msgType) {
      case MSG_TYPE_RESPONSE:
      case MSG_TYPE_ERROR: {
        const response = payload as IpcResponse;
        const pending = this.pendingRequests.get(response.correlation_id);
        if (pending) {
          this.pendingRequests.delete(response.correlation_id);
          if (pending.timer) clearTimeout(pending.timer);

          if (msgType === MSG_TYPE_ERROR || !response.success) {
            pending.reject(
              new ServerError(
                response.error || "Unknown error",
                response.error_code || "UNKNOWN",
              ),
            );
          } else {
            pending.resolve(response);
          }
        }
        break;
      }

      case MSG_TYPE_STREAM: {
        const frame = payload as StreamFrame;
        const stream = this.streamHandlers.get(frame.correlation_id);
        if (stream) {
          stream.frames.push(frame);
          // Notify stream iterator
          const handler = stream as { onFrame?: () => void };
          if (handler.onFrame) {
            handler.onFrame();
          }
          if (frame.is_final) {
            stream.resolve(stream.frames);
          }
        }
        break;
      }

      case MSG_TYPE_PUSH: {
        const notification = payload as PushNotification;
        for (const handler of this.pushHandlers) {
          try {
            handler(notification);
          } catch (err) {
            console.error("Push handler error:", err);
          }
        }
        break;
      }

      case MSG_TYPE_HEARTBEAT:
        // Heartbeat received - no action needed
        break;

      default:
        console.warn(`Unknown message type: ${msgType}`);
    }
  }
}
