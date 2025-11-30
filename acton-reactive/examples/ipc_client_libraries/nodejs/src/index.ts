/**
 * Acton IPC Client Library for Node.js/TypeScript
 *
 * A TypeScript client for communicating with acton-reactive IPC servers.
 * Supports JSON and MessagePack serialization, request-response,
 * streaming, subscriptions, and service discovery.
 *
 * Protocol: v2 with 7-byte header
 * - 4 bytes: payload length (big-endian u32)
 * - 1 byte: protocol version (0x02)
 * - 1 byte: message type
 * - 1 byte: format (0x01 = JSON, 0x02 = MessagePack)
 *
 * @example
 * ```typescript
 * import { ActonIpcClient } from 'acton-ipc-client';
 *
 * const client = new ActonIpcClient('/path/to/socket');
 * await client.connect();
 *
 * const response = await client.request('agent', 'MessageType', { data: 'value' });
 * console.log(response.payload);
 *
 * await client.close();
 * ```
 */

import { createConnection, Socket } from 'net';
import { EventEmitter } from 'events';
import { existsSync } from 'fs';
import { join } from 'path';
import { randomUUID } from 'crypto';

// =============================================================================
// Protocol Constants
// =============================================================================

export enum ProtocolVersion {
  V1 = 0x01, // Legacy: 6-byte header, JSON only
  V2 = 0x02, // Current: 7-byte header, multi-format
}

export enum MessageType {
  REQUEST = 0x01,
  RESPONSE = 0x02,
  ERROR = 0x03,
  HEARTBEAT = 0x04,
  PUSH = 0x05,
  SUBSCRIBE = 0x06,
  UNSUBSCRIBE = 0x07,
  DISCOVER = 0x08,
  STREAM = 0x09,
}

export enum Format {
  JSON = 0x01,
  MESSAGEPACK = 0x02,
}

// Header sizes
const HEADER_SIZE_V1 = 6;
const HEADER_SIZE_V2 = 7;
const MAX_FRAME_SIZE = 16 * 1024 * 1024; // 16 MiB
const DEFAULT_TIMEOUT_MS = 30000;

// =============================================================================
// Error Types
// =============================================================================

export class IpcError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'IpcError';
  }
}

export class ConnectionError extends IpcError {
  constructor(message: string) {
    super(message);
    this.name = 'ConnectionError';
  }
}

export class ProtocolError extends IpcError {
  constructor(message: string) {
    super(message);
    this.name = 'ProtocolError';
  }
}

export class TimeoutError extends IpcError {
  constructor(message: string) {
    super(message);
    this.name = 'TimeoutError';
  }
}

export class ServerError extends IpcError {
  public readonly code?: string;

  constructor(message: string, code?: string) {
    super(message);
    this.name = 'ServerError';
    this.code = code;
  }
}

// =============================================================================
// Type Definitions
// =============================================================================

export interface IpcEnvelope {
  correlation_id: string;
  target: string;
  message_type: string;
  payload: unknown;
  expects_reply: boolean;
  expects_stream: boolean;
  response_timeout_ms: number;
}

export interface IpcResponse {
  correlation_id: string;
  success: boolean;
  payload?: unknown;
  error?: string;
  error_code?: string;
}

export interface StreamFrame {
  correlation_id: string;
  sequence: number;
  payload?: unknown;
  is_final: boolean;
  error?: string;
  error_code?: string;
}

export interface PushNotification {
  notification_id: string;
  message_type: string;
  payload: unknown;
  source_agent?: string;
  timestamp_ms: number;
}

export interface DiscoveryResponse {
  protocol_version: {
    current: number;
    min_supported: number;
    max_supported: number;
    description: string;
    capabilities: {
      messagepack: boolean;
      streaming: boolean;
      push: boolean;
      discovery: boolean;
    };
  };
  agents: Array<{
    name: string;
    ern?: string;
  }>;
  message_types: string[];
}

export interface ClientOptions {
  useMessagePack?: boolean;
  protocolVersion?: ProtocolVersion;
}

// =============================================================================
// Serialization
// =============================================================================

interface Serializer {
  formatByte: Format;
  serialize(data: unknown): Buffer;
  deserialize(data: Buffer): unknown;
}

class JsonSerializer implements Serializer {
  formatByte = Format.JSON;

  serialize(data: unknown): Buffer {
    return Buffer.from(JSON.stringify(data), 'utf-8');
  }

  deserialize(data: Buffer): unknown {
    return JSON.parse(data.toString('utf-8'));
  }
}

class MessagePackSerializer implements Serializer {
  formatByte = Format.MESSAGEPACK;
  private msgpack: typeof import('@msgpack/msgpack') | null = null;

  constructor() {
    try {
      // Dynamic import for optional dependency
      this.msgpack = require('@msgpack/msgpack');
    } catch {
      throw new Error(
        'MessagePack support requires @msgpack/msgpack package. ' +
        'Install with: npm install @msgpack/msgpack'
      );
    }
  }

  serialize(data: unknown): Buffer {
    if (!this.msgpack) throw new Error('MessagePack not available');
    return Buffer.from(this.msgpack.encode(data));
  }

  deserialize(data: Buffer): unknown {
    if (!this.msgpack) throw new Error('MessagePack not available');
    return this.msgpack.decode(data);
  }
}

// =============================================================================
// Correlation ID Generation
// =============================================================================

function generateCorrelationId(prefix: string = 'req'): string {
  // Use timestamp + random for ordering similar to UUIDv7
  const timestampMs = Date.now();
  const timestampHex = timestampMs.toString(16).padStart(12, '0');
  const randomPart = randomUUID().replace(/-/g, '').substring(0, 16);
  return `${prefix}_${timestampHex}${randomPart}`;
}

// =============================================================================
// Frame Encoding/Decoding
// =============================================================================

interface Frame {
  version: ProtocolVersion;
  messageType: MessageType;
  format: Format;
  payload: Buffer;
}

function encodeFrame(
  messageType: MessageType,
  payload: Buffer,
  format: Format = Format.JSON,
  version: ProtocolVersion = ProtocolVersion.V2
): Buffer {
  const payloadLen = payload.length;

  if (payloadLen > MAX_FRAME_SIZE) {
    throw new ProtocolError(`Payload too large: ${payloadLen} > ${MAX_FRAME_SIZE}`);
  }

  if (version === ProtocolVersion.V2) {
    // V2: 7-byte header
    const header = Buffer.alloc(7);
    header.writeUInt32BE(payloadLen, 0);
    header.writeUInt8(version, 4);
    header.writeUInt8(messageType, 5);
    header.writeUInt8(format, 6);
    return Buffer.concat([header, payload]);
  } else {
    // V1: 6-byte header
    const header = Buffer.alloc(6);
    header.writeUInt32BE(payloadLen, 0);
    header.writeUInt8(version, 4);
    header.writeUInt8(messageType, 5);
    return Buffer.concat([header, payload]);
  }
}

// =============================================================================
// Socket Path Resolution
// =============================================================================

export function getDefaultSocketPath(appName: string): string {
  const runtimeDir = process.env.XDG_RUNTIME_DIR || '/tmp';
  return join(runtimeDir, 'acton', appName, 'ipc.sock');
}

export function socketExists(path: string): boolean {
  return existsSync(path);
}

// =============================================================================
// Async IPC Client
// =============================================================================

export class ActonIpcClient extends EventEmitter {
  private socketPath: string;
  private socket: Socket | null = null;
  private connected = false;
  private serializer: Serializer;
  private protocolVersion: ProtocolVersion;

  private pending = new Map<string, {
    resolve: (value: IpcResponse) => void;
    reject: (error: Error) => void;
    timeout: NodeJS.Timeout;
  }>();

  private streamQueues = new Map<string, {
    frames: StreamFrame[];
    resolve: ((frame: StreamFrame) => void) | null;
  }>();

  private receiveBuffer = Buffer.alloc(0);

  constructor(socketPath: string, options: ClientOptions = {}) {
    super();
    this.socketPath = socketPath;
    this.protocolVersion = options.protocolVersion ?? ProtocolVersion.V2;

    if (options.useMessagePack) {
      this.serializer = new MessagePackSerializer();
    } else {
      this.serializer = new JsonSerializer();
    }
  }

  async connect(): Promise<void> {
    if (this.connected) return;

    if (!socketExists(this.socketPath)) {
      throw new ConnectionError(`Socket not found: ${this.socketPath}`);
    }

    return new Promise((resolve, reject) => {
      this.socket = createConnection(this.socketPath, () => {
        this.connected = true;
        resolve();
      });

      this.socket.on('error', (err) => {
        if (!this.connected) {
          reject(new ConnectionError(err.message));
        } else {
          this.handleError(err);
        }
      });

      this.socket.on('data', (data) => {
        this.handleData(data);
      });

      this.socket.on('close', () => {
        this.handleClose();
      });
    });
  }

  async close(): Promise<void> {
    if (this.socket) {
      this.socket.destroy();
      this.socket = null;
    }
    this.connected = false;

    // Reject all pending requests
    for (const [id, pending] of this.pending) {
      clearTimeout(pending.timeout);
      pending.reject(new ConnectionError('Connection closed'));
    }
    this.pending.clear();
  }

  private handleData(data: Buffer): void {
    this.receiveBuffer = Buffer.concat([this.receiveBuffer, data]);
    this.processBuffer();
  }

  private processBuffer(): void {
    while (this.receiveBuffer.length >= 6) {
      // Read header
      const payloadLen = this.receiveBuffer.readUInt32BE(0);
      const version = this.receiveBuffer.readUInt8(4) as ProtocolVersion;

      const headerSize = version === ProtocolVersion.V2 ? HEADER_SIZE_V2 : HEADER_SIZE_V1;
      const totalSize = headerSize + payloadLen;

      if (this.receiveBuffer.length < totalSize) {
        // Not enough data yet
        break;
      }

      // Parse frame
      const messageType = this.receiveBuffer.readUInt8(5) as MessageType;
      const format = version === ProtocolVersion.V2
        ? this.receiveBuffer.readUInt8(6) as Format
        : Format.JSON;

      const payload = this.receiveBuffer.subarray(headerSize, totalSize);
      this.receiveBuffer = this.receiveBuffer.subarray(totalSize);

      // Handle frame
      this.handleFrame({
        version,
        messageType,
        format,
        payload,
      });
    }
  }

  private handleFrame(frame: Frame): void {
    let data: unknown;
    try {
      if (frame.format === Format.MESSAGEPACK) {
        data = new MessagePackSerializer().deserialize(frame.payload);
      } else {
        data = new JsonSerializer().deserialize(frame.payload);
      }
    } catch (err) {
      console.error('Failed to deserialize frame:', err);
      return;
    }

    const response = data as Record<string, unknown>;

    switch (frame.messageType) {
      case MessageType.RESPONSE:
        this.handleResponse(response);
        break;
      case MessageType.ERROR:
        this.handleErrorResponse(response);
        break;
      case MessageType.STREAM:
        this.handleStreamFrame(response);
        break;
      case MessageType.PUSH:
        this.handlePush(response);
        break;
      case MessageType.HEARTBEAT:
        // Heartbeat acknowledged
        break;
    }
  }

  private handleResponse(data: Record<string, unknown>): void {
    const correlationId = data.correlation_id as string;
    const pending = this.pending.get(correlationId);

    if (pending) {
      this.pending.delete(correlationId);
      clearTimeout(pending.timeout);
      pending.resolve(data as unknown as IpcResponse);
    }
  }

  private handleErrorResponse(data: Record<string, unknown>): void {
    const correlationId = data.correlation_id as string;
    const pending = this.pending.get(correlationId);

    if (pending) {
      this.pending.delete(correlationId);
      clearTimeout(pending.timeout);
      pending.reject(new ServerError(
        data.error as string || 'Unknown error',
        data.error_code as string | undefined
      ));
    }
  }

  private handleStreamFrame(data: Record<string, unknown>): void {
    const correlationId = data.correlation_id as string;
    const queue = this.streamQueues.get(correlationId);

    if (queue) {
      const frame: StreamFrame = {
        correlation_id: correlationId,
        sequence: data.sequence as number,
        payload: data.payload,
        is_final: data.is_final as boolean || false,
        error: data.error as string | undefined,
        error_code: data.error_code as string | undefined,
      };

      if (queue.resolve) {
        queue.resolve(frame);
        queue.resolve = null;
      } else {
        queue.frames.push(frame);
      }
    }
  }

  private handlePush(data: Record<string, unknown>): void {
    const notification: PushNotification = {
      notification_id: data.notification_id as string,
      message_type: data.message_type as string,
      payload: data.payload,
      source_agent: data.source_agent as string | undefined,
      timestamp_ms: data.timestamp_ms as number || Date.now(),
    };

    this.emit('push', notification);
  }

  private handleError(err: Error): void {
    // Fail all pending requests
    for (const [id, pending] of this.pending) {
      clearTimeout(pending.timeout);
      pending.reject(new ConnectionError(err.message));
    }
    this.pending.clear();

    this.emit('error', err);
  }

  private handleClose(): void {
    this.connected = false;
    this.emit('close');
  }

  private async sendFrame(messageType: MessageType, data: unknown): Promise<void> {
    if (!this.socket || !this.connected) {
      throw new ConnectionError('Not connected');
    }

    const payload = this.serializer.serialize(data);
    const frame = encodeFrame(
      messageType,
      payload,
      this.serializer.formatByte,
      this.protocolVersion
    );

    return new Promise((resolve, reject) => {
      this.socket!.write(frame, (err) => {
        if (err) reject(new ConnectionError(err.message));
        else resolve();
      });
    });
  }

  async request(
    target: string,
    messageType: string,
    payload: unknown,
    timeoutMs: number = DEFAULT_TIMEOUT_MS
  ): Promise<IpcResponse> {
    const correlationId = generateCorrelationId('req');

    const envelope: IpcEnvelope = {
      correlation_id: correlationId,
      target,
      message_type: messageType,
      payload,
      expects_reply: true,
      expects_stream: false,
      response_timeout_ms: timeoutMs,
    };

    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.pending.delete(correlationId);
        reject(new TimeoutError(`Request timed out after ${timeoutMs}ms`));
      }, timeoutMs);

      this.pending.set(correlationId, { resolve, reject, timeout });

      this.sendFrame(MessageType.REQUEST, envelope).catch((err) => {
        this.pending.delete(correlationId);
        clearTimeout(timeout);
        reject(err);
      });
    });
  }

  async fireAndForget(
    target: string,
    messageType: string,
    payload: unknown
  ): Promise<void> {
    const correlationId = generateCorrelationId('req');

    const envelope: IpcEnvelope = {
      correlation_id: correlationId,
      target,
      message_type: messageType,
      payload,
      expects_reply: false,
      expects_stream: false,
      response_timeout_ms: 0,
    };

    await this.sendFrame(MessageType.REQUEST, envelope);
  }

  async *stream(
    target: string,
    messageType: string,
    payload: unknown,
    timeoutMs: number = DEFAULT_TIMEOUT_MS
  ): AsyncGenerator<StreamFrame> {
    const correlationId = generateCorrelationId('str');

    const envelope: IpcEnvelope = {
      correlation_id: correlationId,
      target,
      message_type: messageType,
      payload,
      expects_reply: false,
      expects_stream: true,
      response_timeout_ms: timeoutMs,
    };

    // Set up stream queue
    this.streamQueues.set(correlationId, { frames: [], resolve: null });

    try {
      await this.sendFrame(MessageType.REQUEST, envelope);

      const startTime = Date.now();

      while (true) {
        const remaining = timeoutMs - (Date.now() - startTime);
        if (remaining <= 0) {
          throw new TimeoutError('Stream timed out');
        }

        const frame = await this.getNextStreamFrame(correlationId, remaining);

        yield frame;

        if (frame.is_final) break;
        if (frame.error) {
          throw new ServerError(frame.error, frame.error_code);
        }
      }
    } finally {
      this.streamQueues.delete(correlationId);
    }
  }

  private getNextStreamFrame(correlationId: string, timeoutMs: number): Promise<StreamFrame> {
    return new Promise((resolve, reject) => {
      const queue = this.streamQueues.get(correlationId);
      if (!queue) {
        reject(new Error('Stream queue not found'));
        return;
      }

      // Check if frame already available
      if (queue.frames.length > 0) {
        resolve(queue.frames.shift()!);
        return;
      }

      // Wait for frame
      const timeout = setTimeout(() => {
        if (queue.resolve === resolve) {
          queue.resolve = null;
          reject(new TimeoutError('Stream timed out'));
        }
      }, timeoutMs);

      queue.resolve = (frame) => {
        clearTimeout(timeout);
        resolve(frame);
      };
    });
  }

  async subscribe(messageTypes: string[]): Promise<boolean> {
    const correlationId = generateCorrelationId('sub');

    const data = {
      correlation_id: correlationId,
      message_types: messageTypes,
    };

    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.pending.delete(correlationId);
        resolve(false);
      }, 5000);

      this.pending.set(correlationId, {
        resolve: (response) => {
          resolve(response.success);
        },
        reject,
        timeout,
      });

      this.sendFrame(MessageType.SUBSCRIBE, data).catch((err) => {
        this.pending.delete(correlationId);
        clearTimeout(timeout);
        resolve(false);
      });
    });
  }

  async unsubscribe(messageTypes: string[]): Promise<boolean> {
    const correlationId = generateCorrelationId('unsub');

    const data = {
      correlation_id: correlationId,
      message_types: messageTypes,
    };

    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.pending.delete(correlationId);
        resolve(false);
      }, 5000);

      this.pending.set(correlationId, {
        resolve: (response) => {
          resolve(response.success);
        },
        reject,
        timeout,
      });

      this.sendFrame(MessageType.UNSUBSCRIBE, data).catch((err) => {
        this.pending.delete(correlationId);
        clearTimeout(timeout);
        resolve(false);
      });
    });
  }

  onPush(handler: (notification: PushNotification) => void): void {
    this.on('push', handler);
  }

  async discover(): Promise<DiscoveryResponse> {
    const correlationId = generateCorrelationId('disc');

    const data = {
      correlation_id: correlationId,
      include_agents: true,
      include_message_types: true,
    };

    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.pending.delete(correlationId);
        reject(new TimeoutError('Discovery timed out'));
      }, 5000);

      this.pending.set(correlationId, {
        resolve: (response) => {
          resolve(response.payload as DiscoveryResponse);
        },
        reject,
        timeout,
      });

      this.sendFrame(MessageType.DISCOVER, data).catch((err) => {
        this.pending.delete(correlationId);
        clearTimeout(timeout);
        reject(err);
      });
    });
  }

  async heartbeat(): Promise<void> {
    await this.sendFrame(MessageType.HEARTBEAT, {});
  }
}

// =============================================================================
// Exports
// =============================================================================

export default ActonIpcClient;
