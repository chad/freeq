/**
 * E2E tests: Mobile viewport behavior
 *
 * Uses iPhone 14 viewport (390×844) via Chromium emulation.
 */
import { test, expect } from '@playwright/test';
import { uniqueNick, uniqueChannel, connectGuest, sendMessage, expectMessage } from './helpers';

function isMobileViewport(page: any): boolean {
  const vp = page.viewportSize();
  return vp && vp.width <= 500;
}

test.describe('Mobile', () => {
  test('login screen fits mobile viewport', async ({ page }) => {
    test.skip(!isMobileViewport(page), 'mobile only');
    await page.goto('/');

    const loginCard = page.locator('[class*="rounded-2xl"]').first();
    await expect(loginCard).toBeVisible();

    // Check nothing overflows horizontally
    const vp = page.viewportSize()!;
    const bodyWidth = await page.evaluate(() => document.body.scrollWidth);
    expect(bodyWidth).toBeLessThanOrEqual(vp.width + 5);
  });

  test('can connect as guest on mobile', async ({ page }) => {
    test.skip(!isMobileViewport(page), 'mobile only');
    const nick = uniqueNick('mob');
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);
  });

  test('compose box is accessible on mobile', async ({ page }) => {
    test.skip(!isMobileViewport(page), 'mobile only');
    const nick = uniqueNick('mob');
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    const compose = page.getByTestId('compose-input');
    await expect(compose).toBeVisible();

    await sendMessage(page, 'mobile test message');
    await expectMessage(page, 'mobile test message');
  });

  test('sidebar accessible on mobile', async ({ page }) => {
    test.skip(!isMobileViewport(page), 'mobile only');
    const nick = uniqueNick('mob');
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    // On mobile, sidebar might be hidden — look for toggle button
    const sidebar = page.getByTestId('sidebar');
    const sidebarVisible = await sidebar.isVisible().catch(() => false);

    if (!sidebarVisible) {
      // Try hamburger/menu buttons
      const menuBtn = page.locator('button[title*="enu"], button[title*="idebar"], [data-testid="sidebar-toggle"]');
      if (await menuBtn.count() > 0) {
        await menuBtn.first().click();
        await expect(page.getByText(channel)).toBeVisible({ timeout: 3_000 });
      }
    } else {
      await expect(sidebar.getByText(channel)).toBeVisible();
    }
  });

  test('no horizontal overflow on mobile', async ({ page }) => {
    test.skip(!isMobileViewport(page), 'mobile only');
    const nick = uniqueNick('mob');
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    await sendMessage(page, 'short msg');
    await sendMessage(page, 'a longer message that might wrap on a small screen but should not cause horizontal scroll');
    await sendMessage(page, 'https://example.com/a-very-long-url-that-could-potentially-break-layout-on-mobile-devices.html');

    const vp = page.viewportSize()!;
    const bodyWidth = await page.evaluate(() => document.body.scrollWidth);
    expect(bodyWidth).toBeLessThanOrEqual(vp.width + 5);
  });

  test('messages are readable on mobile', async ({ page }) => {
    test.skip(!isMobileViewport(page), 'mobile only');
    const nick = uniqueNick('mob');
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    await sendMessage(page, 'can you read this on mobile?');
    await expectMessage(page, 'can you read this on mobile?');

    // Text should be large enough (at least 12px)
    const fontSize = await page.locator('.text-sm').first().evaluate(
      (el: HTMLElement) => parseFloat(getComputedStyle(el).fontSize)
    );
    expect(fontSize).toBeGreaterThanOrEqual(12);
  });
});
