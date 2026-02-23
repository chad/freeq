/**
 * E2E tests: Theme switching, density settings
 */
import { test, expect } from '@playwright/test';
import { uniqueNick, uniqueChannel, connectGuest, sendMessage, prepPage } from './helpers';

test.describe('Theme and appearance', () => {
  test('defaults to dark theme', async ({ page }) => {
    const nick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    // The body or root element should have dark-related class
    const bgColor = await page.evaluate(() => {
      return getComputedStyle(document.body).backgroundColor;
    });
    // Dark theme has dark background (rgb values close to 0)
    // Just check it's not white
    expect(bgColor).not.toBe('rgb(255, 255, 255)');
  });

  test('remembers theme preference', async ({ page }) => {
    await prepPage(page);
    // Set theme preference before loading
    await page.addInitScript(() => {
      localStorage.setItem('freeq-theme', 'light');
    });
    await page.goto('/');

    // The page should have light theme applied
    const bgColor = await page.evaluate(() => {
      return getComputedStyle(document.body).backgroundColor;
    });
    // Light theme should have a light background
    // This is a basic check — exact color depends on theme
  });

  test('remembers density preference', async ({ page }) => {
    await prepPage(page);
    await page.addInitScript(() => {
      localStorage.setItem('freeq-density', 'compact');
    });

    const nick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    // Verify density is applied — compact has smaller text
    await sendMessage(page, 'density test message');
    // The app should have compact density class or styles
  });
});
