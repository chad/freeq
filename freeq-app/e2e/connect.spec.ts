/**
 * E2E tests: Connection + Login flow
 */
import { test, expect } from '@playwright/test';
import { uniqueNick, uniqueChannel, connectGuest, prepPage } from './helpers';

test.describe('Connection', () => {
  test('shows login screen on first load', async ({ page }) => {
    await page.goto('/');
    await expect(page.getByRole('heading', { name: 'freeq' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'AT Protocol', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Guest' })).toBeVisible();
  });

  test('AT Protocol tab is default', async ({ page }) => {
    await page.goto('/');
    const atTab = page.getByRole('button', { name: 'AT Protocol', exact: true });
    await expect(atTab).toHaveClass(/text-accent/);
    await expect(page.getByPlaceholder('you.bsky.social')).toBeVisible();
  });

  test('guest login connects and joins channel', async ({ page }) => {
    const nick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);
    await expect(page.getByTestId('sidebar').getByText(channel)).toBeVisible();
  });

  test('shows error for empty guest nick', async ({ page }) => {
    await page.goto('/');
    await page.getByRole('button', { name: 'Guest' }).click();
    const nickInput = page.getByPlaceholder('your_nick');
    await nickInput.fill('');
    await page.getByRole('button', { name: 'Connect as Guest' }).click();
    await expect(page.getByText('Enter a nickname')).toBeVisible();
  });

  test('shows advanced settings', async ({ page }) => {
    await page.goto('/');
    await page.getByText('Advanced settings').click();
    await expect(page.getByText('WebSocket URL')).toBeVisible();
    await expect(page.getByText('Server HTTP Origin')).toBeVisible();
  });

  test('guest can join multiple channels on connect', async ({ page }) => {
    const nick = uniqueNick();
    const ch1 = uniqueChannel();
    const ch2 = uniqueChannel();
    await connectGuest(page, nick, `${ch1}, ${ch2}`);
    const sidebar = page.getByTestId('sidebar');
    await expect(sidebar.getByText(ch1)).toBeVisible({ timeout: 10_000 });
    await expect(sidebar.getByText(ch2)).toBeVisible({ timeout: 10_000 });
  });
});
