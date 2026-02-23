/**
 * E2E tests: Mobile viewport behavior
 *
 * Uses iPhone 14 viewport (390×844) via Chromium emulation.
 */
import { test, expect } from '@playwright/test';
import { uniqueNick, uniqueChannel, connectGuest, sendMessage, expectMessage, openSidebar } from './helpers';

function isMobileViewport(page: any): boolean {
  const vp = page.viewportSize();
  return vp && vp.width <= 500;
}

test.describe('Mobile', () => {
  test('login screen fits mobile viewport', async ({ page }) => {
    test.skip(!isMobileViewport(page), 'mobile only');
    await page.goto('/');

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

  test('sidebar opens via hamburger on mobile', async ({ page }) => {
    test.skip(!isMobileViewport(page), 'mobile only');
    const nick = uniqueNick('mob');
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    const sidebar = await openSidebar(page);
    await expect(sidebar.getByText(channel)).toBeVisible({ timeout: 3_000 });
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

    const fontSize = await page.locator('.text-sm').first().evaluate(
      (el: HTMLElement) => parseFloat(getComputedStyle(el).fontSize)
    );
    expect(fontSize).toBeGreaterThanOrEqual(12);
  });

  test('two users can chat on mobile viewports', async ({ browser }) => {
    const nick1 = uniqueNick('mob1');
    const nick2 = uniqueNick('mob2');
    const channel = uniqueChannel();

    const ctx1 = await browser.newContext({
      viewport: { width: 390, height: 844 },
      isMobile: true,
    });
    const ctx2 = await browser.newContext({
      viewport: { width: 390, height: 844 },
      isMobile: true,
    });
    const page1 = await ctx1.newPage();
    const page2 = await ctx2.newPage();

    await connectGuest(page1, nick1, channel);
    await connectGuest(page2, nick2, channel);
    await page1.waitForTimeout(500);

    await sendMessage(page1, `hello from mobile ${nick1}`);
    await expectMessage(page2, `hello from mobile ${nick1}`);

    await sendMessage(page2, `reply from mobile ${nick2}`);
    await expectMessage(page1, `reply from mobile ${nick2}`);

    await ctx1.close();
    await ctx2.close();
  });
});
