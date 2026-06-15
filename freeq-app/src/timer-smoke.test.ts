import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.clearAllTimers();
  vi.restoreAllMocks();
  // Note: do NOT call vi.useRealTimers() as it hangs in vitest 4.1.x on node 18
  // vitest resets timers between test files automatically
});

describe('timer test', () => {
  it('basic', () => {
    expect(1 + 1).toBe(2);
  });
});
