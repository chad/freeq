/**
 * Screenshot capture for mobile UX evaluation.
 * Not a real test — just captures screenshots.
 */
import { test } from '@playwright/test';
import { uniqueNick, uniqueChannel, connectGuest, sendMessage } from './helpers';

test.describe('Screenshots', () => {
  test('mobile login screen', async ({ page }) => {
    const vp = page.viewportSize();
    test.skip(!vp || vp.width > 500, 'mobile only');
    await page.goto('/');
    await page.waitForTimeout(500);
    await page.screenshot({ path: 'screenshots/mobile-login.png', fullPage: true });
  });

  test('mobile chat view', async ({ page }) => {
    const vp = page.viewportSize();
    test.skip(!vp || vp.width > 500, 'mobile only');
    const nick = uniqueNick('mob');
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);
    await sendMessage(page, 'Hello from mobile! 👋');
    await sendMessage(page, 'Testing the mobile layout');
    await sendMessage(page, 'https://bsky.app/profile/chadfowler.com');
    await page.waitForTimeout(1000);
    await page.screenshot({ path: 'screenshots/mobile-chat.png', fullPage: true });
  });

  test('desktop chat view', async ({ page }) => {
    const vp = page.viewportSize();
    test.skip(!vp || vp.width <= 500, 'desktop only');
    const nick = uniqueNick('desk');
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);
    await sendMessage(page, 'Hello from desktop! 👋');
    await sendMessage(page, 'Testing the desktop layout with a longer message to see how wrapping works');
    await page.waitForTimeout(1000);
    await page.screenshot({ path: 'screenshots/desktop-chat.png', fullPage: true });
  });

  test('desktop login screen', async ({ page }) => {
    const vp = page.viewportSize();
    test.skip(!vp || vp.width <= 500, 'desktop only');
    await page.goto('/');
    await page.waitForTimeout(500);
    await page.screenshot({ path: 'screenshots/desktop-login.png', fullPage: true });
  });
});
