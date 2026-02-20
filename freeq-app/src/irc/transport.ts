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
      this.opts.onStateChange('connected');
    };

    this.ws.onmessage = (e: MessageEvent) => {
      const data = typeof e.data === 'string' ? e.data : '';
      for (const line of data.split('\n')) {
        const trimmed = line.replace(/\r$/, '');
        if (trimmed) this.opts.onLine(trimmed);
      }
    };

    this.ws.onclose = () => {
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
      this.ws.send(line);
    }
  }

  disconnect() {
    this.intentionalClose = true;
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
