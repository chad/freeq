import { useState, useEffect, useCallback } from 'react';
import { useStore } from '../store';
import { showToast } from './Toast';

export function FileDropOverlay() {
  const [dragging, setDragging] = useState(false);
  const activeChannel = useStore((s) => s.activeChannel);
  const authDid = useStore((s) => s.authDid);

  const handleDragEnter = useCallback((e: DragEvent) => {
    e.preventDefault();
    if (e.dataTransfer?.types.includes('Files')) {
      setDragging(true);
    }
  }, []);

  const handleDragLeave = useCallback((e: DragEvent) => {
    e.preventDefault();
    // Only dismiss if leaving the window
    if (e.relatedTarget === null) {
      setDragging(false);
    }
  }, []);

  const handleDragOver = useCallback((e: DragEvent) => {
    e.preventDefault();
  }, []);

  const handleDrop = useCallback((e: DragEvent) => {
    e.preventDefault();
    setDragging(false);
    const files = e.dataTransfer?.files;
    if (!files || files.length === 0) return;
    if (!authDid) {
      showToast('Sign in with AT Protocol to upload files', 'warning');
      return;
    }
    if (!activeChannel || activeChannel === 'server') {
      showToast('Switch to a channel to upload files', 'warning');
      return;
    }
    // Dispatch custom event that ComposeBox listens for
    window.dispatchEvent(new CustomEvent('freeq-file-drop', { detail: { file: files[0] } }));
  }, [authDid, activeChannel]);

  useEffect(() => {
    window.addEventListener('dragenter', handleDragEnter);
    window.addEventListener('dragleave', handleDragLeave);
    window.addEventListener('dragover', handleDragOver);
    window.addEventListener('drop', handleDrop);
    return () => {
      window.removeEventListener('dragenter', handleDragEnter);
      window.removeEventListener('dragleave', handleDragLeave);
      window.removeEventListener('dragover', handleDragOver);
      window.removeEventListener('drop', handleDrop);
    };
  }, [handleDragEnter, handleDragLeave, handleDragOver, handleDrop]);

  if (!dragging) return null;

  return (
    <div className="fixed inset-0 z-[300] bg-bg/80 backdrop-blur-sm flex items-center justify-center pointer-events-none">
      <div className="border-2 border-dashed border-accent rounded-2xl p-12 bg-accent/5 animate-pulse">
        <div className="text-center">
          <svg className="w-16 h-16 mx-auto text-accent mb-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
            <path d="M12 16V4m0 0L8 8m4-4l4 4" />
            <path d="M20 16.7V19a2 2 0 01-2 2H6a2 2 0 01-2-2v-2.3" />
          </svg>
          <div className="text-xl font-bold text-accent">Drop file to upload</div>
          <div className="text-sm text-fg-dim mt-1">to {activeChannel}</div>
        </div>
      </div>
    </div>
  );
}
