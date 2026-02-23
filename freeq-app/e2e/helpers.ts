/**
 * Shared helpers for freeq E2E tests.
 */
import { Page, expect } from '@playwright/test';

const TS = Date.now().toString(36);
let counter = 0;

/** Generate a unique nick for each test user */
export function uniqueNick(prefix = 'pw'): string {
  return `${prefix}${TS}${counter++}`;
}

/** Generate a unique channel name */
export function uniqueChannel(): string {
  return `#pw-${TS}-${counter++}`;
}

/** Connect as guest and wait for registration */
export async function connectGuest(page: Page, nick: string, channel: string) {
  // Dismiss onboarding tour and install prompt for clean tests
  await page.addInitScript(() => {
    localStorage.setItem('freeq-onboarding-done', '1');
    localStorage.setItem('freeq-install-dismissed', '1');
  });
  await page.goto('/');

  // Switch to Guest tab
  const guestTab = page.getByRole('button', { name: 'Guest' });
  await guestTab.click();

  // Fill nickname
  const nickInput = page.getByPlaceholder('your_nick');
  await nickInput.fill(nick);

  // Fill channel
  const channelInput = page.getByPlaceholder('#freeq');
  await channelInput.fill(channel);

  // Click connect
  await page.getByRole('button', { name: 'Connect as Guest' }).click();

  // Wait for the chat UI to appear (sidebar with channel name)
  await expect(page.getByTestId('sidebar')).toBeVisible({ timeout: 15_000 });

  // Wait for first channel to appear in sidebar
  // channel might be comma-separated — just check the first one
  const firstChannel = channel.split(',')[0].trim();
  await expect(page.getByTestId('sidebar').getByText(firstChannel)).toBeVisible({ timeout: 10_000 });
}

/** Send a message in the compose box */
export async function sendMessage(page: Page, text: string) {
  const compose = page.getByTestId('compose-input');
  await compose.click();
  await compose.fill(text);
  // Dismiss any autocomplete popup that may have appeared
  await compose.press('Escape');
  await page.waitForTimeout(50);
  await compose.press('Enter');
  // Wait for compose to clear (message was sent)
  await expect(compose).toHaveValue('', { timeout: 3_000 });
}

/** Wait for a message to appear in the message list */
export async function expectMessage(page: Page, text: string, timeout = 10_000) {
  await expect(page.getByTestId('message-list').getByText(text, { exact: false })).toBeVisible({ timeout });
}

/** Wait for a system/status message */
export async function expectSystemMessage(page: Page, text: string, timeout = 10_000) {
  await expect(page.getByText(text, { exact: false })).toBeVisible({ timeout });
}
