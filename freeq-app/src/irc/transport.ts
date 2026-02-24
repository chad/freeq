/** WebSocket IRC transport with auto-reconnect. */

export type TransportState = 'disconnected' | 'connecting' | 'connected';

export interface TransportOptions {
  url: string;
  onLine: (line: string) => void;
  onStateChange: (state: TransportState) => void;
}

export class Transport {
  private ws: WebSocket | null = null;
  private opts: TransportOptions;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private reconnectAttempts = 0;
  private intentionalClose = false;
  private lastDataReceived = 0;
  private heartbeatTimer: ReturnType<typeof setInterval> | null = null;

  // If no data received in 45s, send a client-side PING.
  // If no data received in 90s, assume connection is dead and force reconnect.
  private static PING_INTERVAL = 45_000;
  private static DEAD_TIMEOUT = 90_000;

  constructor(opts: TransportOptions) {
    this.opts = opts;
  }

  connect() {
    this.intentionalClose = false;
    this.opts.onStateChange('connecting');

    try {
      this.ws = new WebSocket(this.opts.url);
    } catch {
      this.opts.onStateChange('disconnected');
      this.scheduleReconnect();
      return;
    }

    this.ws.onopen = () => {
      this.reconnectAttempts = 0;
      this.lastDataReceived = Date.now();
      this.opts.onStateChange('connected');
      this.startHeartbeat();
    };

    this.ws.onmessage = (e: MessageEvent) => {
      this.lastDataReceived = Date.now();
      const data = typeof e.data === 'string' ? e.data : '';
      for (const line of data.split('\n')) {
        const trimmed = line.replace(/\r$/, '');
        if (trimmed) this.opts.onLine(trimmed);
      }
    };

    this.ws.onclose = () => {
      this.stopHeartbeat();
      this.opts.onStateChange('disconnected');
      if (!this.intentionalClose) {
        this.scheduleReconnect();
      }
    };

    this.ws.onerror = () => {
      // onclose will fire after this
    };
  }

  send(line: string) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      // If bufferedAmount is high, the connection is likely dead
      if (this.ws.bufferedAmount > 65536) {
        console.warn('[transport] High bufferedAmount, forcing reconnect');
        this.ws.close();
        return;
      }
      this.ws.send(line);
    }
  }

  disconnect() {
    this.intentionalClose = true;
    this.stopHeartbeat();
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.ws) {
      try { this.send('QUIT :Leaving'); } catch { /* ignore */ }
      this.ws.close();
      this.ws = null;
    }
    this.opts.onStateChange('disconnected');
  }

  private startHeartbeat() {
    this.stopHeartbeat();
    this.heartbeatTimer = setInterval(() => {
      const elapsed = Date.now() - this.lastDataReceived;
      if (elapsed > Transport.DEAD_TIMEOUT) {
        // No data for 90s — connection is dead, force reconnect
        console.warn('[transport] No data for 90s, forcing reconnect');
        this.stopHeartbeat();
        if (this.ws) {
          this.ws.close(); // triggers onclose → scheduleReconnect
          this.ws = null;
        }
      } else if (elapsed > Transport.PING_INTERVAL) {
        // No data for 45s — send a client-side PING to provoke a response
        this.send('PING :heartbeat');
      }
    }, 15_000); // Check every 15s
  }

  private stopHeartbeat() {
    if (this.heartbeatTimer) {
      clearInterval(this.heartbeatTimer);
      this.heartbeatTimer = null;
    }
  }

  private scheduleReconnect() {
    if (this.reconnectTimer || this.intentionalClose) return;
    this.reconnectAttempts++;
    const delay = Math.min(1000 * Math.pow(2, this.reconnectAttempts - 1), 30000);
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.connect();
    }, delay);
  }
}
