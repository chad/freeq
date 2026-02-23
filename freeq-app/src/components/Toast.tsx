import { useEffect, useState } from 'react';

export interface ToastData {
  id: string;
  message: string;
  type: 'info' | 'success' | 'error' | 'warning';
  duration?: number;
}

let toastListeners: ((toasts: ToastData[]) => void)[] = [];
let currentToasts: ToastData[] = [];

function emit() {
  toastListeners.forEach((fn) => fn([...currentToasts]));
}

export function showToast(message: string, type: ToastData['type'] = 'info', duration = 4000) {
  const id = Math.random().toString(36).slice(2);
  const toast: ToastData = { id, message, type, duration };
  currentToasts = [...currentToasts, toast];
  emit();
  if (duration > 0) {
    setTimeout(() => dismissToast(id), duration);
  }
}

export function dismissToast(id: string) {
  currentToasts = currentToasts.filter((t) => t.id !== id);
  emit();
}

const TYPE_STYLES: Record<string, string> = {
  info: 'bg-blue/10 border-blue/30 text-blue',
  success: 'bg-success/10 border-success/30 text-success',
  error: 'bg-danger/10 border-danger/30 text-danger',
  warning: 'bg-warning/10 border-warning/30 text-warning',
};

const TYPE_ICONS: Record<string, string> = {
  info: 'ℹ️',
  success: '✅',
  error: '❌',
  warning: '⚠️',
};

export function ToastContainer() {
  const [toasts, setToasts] = useState<ToastData[]>([]);

  useEffect(() => {
    toastListeners.push(setToasts);
    return () => {
      toastListeners = toastListeners.filter((fn) => fn !== setToasts);
    };
  }, []);

  if (toasts.length === 0) return null;

  return (
    <div className="fixed bottom-20 right-4 z-[200] flex flex-col gap-2 max-w-sm">
      {toasts.map((toast) => (
        <div
          key={toast.id}
          className={`flex items-center gap-2 px-4 py-2.5 rounded-xl border shadow-lg backdrop-blur-sm animate-slideIn ${TYPE_STYLES[toast.type]}`}
          onClick={() => dismissToast(toast.id)}
        >
          <span className="text-sm">{TYPE_ICONS[toast.type]}</span>
          <span className="text-sm font-medium flex-1">{toast.message}</span>
          <button className="text-xs opacity-60 hover:opacity-100 ml-2" onClick={() => dismissToast(toast.id)}>✕</button>
        </div>
      ))}
    </div>
  );
}
