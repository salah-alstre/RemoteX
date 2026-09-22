import { h264CodecString, h265CodecString, parsePacket } from "../lib/codec";

export interface PipelineMetrics {
  fps: number;
  decodeMs: number;
  renderMs: number;
  width: number;
  height: number;
  droppedByDecoder: number;
}

interface Hooks {
  /** The stream needs a fresh keyframe (join, decoder overload or lost reference). */
  needKeyframe: () => void;
  /** Codec the local decoder cannot handle; the caller should fall back. */
  unsupported: (codec: string) => void;
  ack: (count: number) => void;
}

/** Frames allowed to wait for the decoder. Interactive use wants the newest picture, not a complete history. */
const MAX_DECODE_QUEUE = 2;

/**
 * Hardware-accelerated decode with WebCodecs, drawn straight onto a canvas.
 * Runs continuously, but only decodes while a canvas is attached.
 */
export class VideoPipeline {
  private decoder: VideoDecoder | null = null;
  private configKey = "";
  private canvas: HTMLCanvasElement | null = null;
  private ctx: CanvasRenderingContext2D | null = null;
  private awaitingKey = true;
  private pendingAcks = 0;
  private ackTimer: number | undefined;
  private submitted = new Map<number, number>();
  private frames = 0;
  private decodeSum = 0;
  private renderSum = 0;
  private samples = 0;
  private lastTick = performance.now();
  private dropped = 0;
  private width = 0;
  private height = 0;
  onSize: (w: number, h: number) => void = () => {};
  private metrics: PipelineMetrics = { fps: 0, decodeMs: 0, renderMs: 0, width: 0, height: 0, droppedByDecoder: 0 };

  constructor(private hooks: Hooks) {}

  attach(canvas: HTMLCanvasElement | null): void {
    this.canvas = canvas;
    this.ctx = canvas ? canvas.getContext("2d", { alpha: false, desynchronized: true }) : null;
    if (canvas) {
      this.awaitingKey = true;
      this.hooks.needKeyframe();
    }
  }

  getMetrics(): PipelineMetrics {
    const now = performance.now();
    const dt = (now - this.lastTick) / 1000;
    if (dt >= 1) {
      this.metrics = {
        fps: this.frames / dt,
        decodeMs: this.samples ? this.decodeSum / this.samples : 0,
        renderMs: this.samples ? this.renderSum / this.samples : 0,
        width: this.width,
        height: this.height,
        droppedByDecoder: this.dropped,
      };
      this.frames = this.samples = 0;
      this.decodeSum = this.renderSum = 0;
      this.lastTick = now;
    }
    return this.metrics;
  }

  private ack(): void {
    this.pendingAcks++;
    if (this.pendingAcks >= 4) return this.flushAcks();
    this.ackTimer ??= window.setTimeout(() => this.flushAcks(), 40);
  }

  private flushAcks(): void {
    window.clearTimeout(this.ackTimer);
    this.ackTimer = undefined;
    if (this.pendingAcks > 0) this.hooks.ack(this.pendingAcks);
    this.pendingAcks = 0;
  }

  private draw(frame: VideoFrame): void {
    const t0 = performance.now();
    const sent = this.submitted.get(frame.timestamp);
    if (sent !== undefined) {
      this.decodeSum += t0 - sent;
      this.submitted.delete(frame.timestamp);
    }
    const canvas = this.canvas;
    if (canvas && this.ctx) {
      if (canvas.width !== frame.displayWidth || canvas.height !== frame.displayHeight) {
        canvas.width = frame.displayWidth;
        canvas.height = frame.displayHeight;
        this.width = frame.displayWidth;
        this.height = frame.displayHeight;
        this.onSize(this.width, this.height);
      }
      this.ctx.drawImage(frame, 0, 0);
    }
    frame.close();
    this.renderSum += performance.now() - t0;
    this.samples++;
    this.frames++;
  }

  private async configure(codec: string, name: string): Promise<boolean> {
    const config: VideoDecoderConfig = {
      codec,
      hardwareAcceleration: "prefer-hardware",
      optimizeForLatency: true,
      colorSpace: { primaries: "bt709", transfer: "bt709", matrix: "bt709", fullRange: false },
    };
    const support = await VideoDecoder.isConfigSupported(config).catch(() => ({ supported: false }));
    if (!support.supported) {
      this.hooks.unsupported(name);
      return false;
    }
    this.decoder?.close();
    this.submitted.clear();
    this.decoder = new VideoDecoder({
      output: (f) => this.draw(f),
      error: () => {
        this.awaitingKey = true;
        this.configKey = "";
        this.hooks.needKeyframe();
      },
    });
    this.decoder.configure(config);
    this.configKey = codec;
    return true;
  }

  async onPacket(buf: ArrayBuffer): Promise<void> {
    const p = parsePacket(buf);
    this.ack();
    if (!p || !this.canvas) {
      this.awaitingKey = true;
      return;
    }
    if (p.keyframe) {
      const codec = p.codec === "h264" ? h264CodecString(p.data) : p.codec === "h265" ? h265CodecString(p.data) : null;
      if (!codec) {
        this.hooks.unsupported(p.codec);
        return;
      }
      if (codec !== this.configKey || this.decoder?.state !== "configured") {
        if (!(await this.configure(codec, p.codec))) return;
      }
      this.awaitingKey = false;
    } else if (this.awaitingKey || this.decoder?.state !== "configured") {
      this.hooks.needKeyframe();
      return;
    }
    const decoder = this.decoder;
    if (!decoder) return;
    if (decoder.decodeQueueSize > MAX_DECODE_QUEUE && !p.keyframe) {
      // The decoder is behind: drop until the next keyframe rather than accumulate latency. Showing an old
      // frame late is worse than showing the next one on time.
      this.awaitingKey = true;
      this.dropped++;
      this.hooks.needKeyframe();
      return;
    }
    this.submitted.set(p.seq, performance.now());
    decoder.decode(new EncodedVideoChunk({ type: p.keyframe ? "key" : "delta", timestamp: p.seq, data: p.data }));
  }

  close(): void {
    this.decoder?.close();
    this.decoder = null;
    this.configKey = "";
    this.flushAcks();
  }
}
