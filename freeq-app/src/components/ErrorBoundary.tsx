import React from 'react';
import { disconnect, reconnect } from '../irc/client';

interface State {
  hasError: boolean;
  error?: Error;
}

export class ErrorBoundary extends React.Component<{ children: React.ReactNode }, State> {
  state: State = { hasError: false };

  static getDerivedStateFromError(error: Error): State {
    return { hasError: true, error };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    console.error('[ErrorBoundary]', error, info.componentStack);
  }

  render() {
    if (!this.state.hasError) return this.props.children;

    return (
      <div className="h-screen bg-bg flex items-center justify-center p-6">
        <div className="max-w-md text-center space-y-4">
          <h1 className="text-2xl font-bold text-fg">Something went wrong</h1>
          <p className="text-fg-dim text-sm">
            {this.state.error?.message || 'An unexpected error occurred.'}
          </p>
          <div className="flex gap-3 justify-center">
            <button
              onClick={() => {
                this.setState({ hasError: false, error: undefined });
                reconnect();
              }}
              className="px-4 py-2 bg-accent text-white rounded hover:opacity-90"
            >
              Reconnect
            </button>
            <button
              onClick={() => {
                disconnect();
                this.setState({ hasError: false, error: undefined });
                window.location.reload();
              }}
              className="px-4 py-2 bg-surface text-fg rounded hover:opacity-90"
            >
              Reload
            </button>
          </div>
          <details className="text-left text-xs text-fg-dim mt-4">
            <summary className="cursor-pointer">Technical details</summary>
            <pre className="mt-2 p-2 bg-surface rounded overflow-auto max-h-40">
              {this.state.error?.stack}
            </pre>
          </details>
        </div>
      </div>
    );
  }
}
