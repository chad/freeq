/**
 * E2E tests: Channel operations
 */
import { test, expect } from '@playwright/test';
import { uniqueNick, uniqueChannel, connectGuest, sendMessage } from './helpers';

test.describe('Channels', () => {
  test('channel topic shows in top bar', async ({ page }) => {
    // Topic is hidden on mobile (sm: breakpoint)
    const vp = page.viewportSize();
    test.skip(!vp || vp.width < 640, 'topic hidden on mobile');

    const nick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    // Set a topic via IRC command
    await sendMessage(page, `/topic ${channel} E2E test topic`);

    // Topic should appear somewhere on page
    await expect(page.getByText('E2E test topic')).toBeVisible({ timeout: 5_000 });
  });

  test('can switch between channels', async ({ page }) => {
    const nick = uniqueNick();
    const ch1 = uniqueChannel();
    const ch2 = uniqueChannel();
    await connectGuest(page, nick, `${ch1}, ${ch2}`);
    const isMobile = (page.viewportSize()?.width || 1280) < 768;

    // Helper to ensure sidebar is visible (open hamburger on mobile)
    const openSidebar = async () => {
      const sidebar = page.getByTestId('sidebar');
      // On mobile, sidebar may be off-screen (translated). Check if we need to open it.
      const isInViewport = await sidebar.evaluate((el) => {
        const rect = el.getBoundingClientRect();
        return rect.right > 0 && rect.left < window.innerWidth;
      }).catch(() => false);

      if (!isInViewport) {
        // Click the hamburger menu button in the top bar
        const hamburger = page.locator('header button').first();
        await hamburger.click();
        await page.waitForTimeout(300); // wait for slide animation
      }
      return sidebar;
    };

    let sidebar = await openSidebar();
    await expect(sidebar.getByText(ch1)).toBeVisible({ timeout: 10_000 });
    await expect(sidebar.getByText(ch2)).toBeVisible({ timeout: 10_000 });

    // Click ch1 to make sure we're there
    await sidebar.getByText(ch1).click();
    await sendMessage(page, 'msg in first channel');
    await expect(page.getByTestId('message-list').getByText('msg in first channel')).toBeVisible();

    // Click ch2 in sidebar (reopen on mobile since it closes on channel switch)
    sidebar = await openSidebar();
    await sidebar.getByText(ch2).click();
    await sendMessage(page, 'msg in second channel');
    await expect(page.getByTestId('message-list').getByText('msg in second channel')).toBeVisible();

    // Switch back to ch1 — should still see first message
    sidebar = await openSidebar();
    await sidebar.getByText(ch1).click();
    await expect(page.getByTestId('message-list').getByText('msg in first channel')).toBeVisible();
  });

  test('join command works', async ({ page }) => {
    const nick = uniqueNick();
    const ch1 = uniqueChannel();
    const ch2 = uniqueChannel();
    await connectGuest(page, nick, ch1);

    // Join a second channel via command
    await sendMessage(page, `/join ${ch2}`);

    // Should appear in sidebar
    await expect(page.getByTestId('sidebar').getByText(ch2)).toBeVisible({ timeout: 5_000 });
  });

  test('part command removes channel', async ({ page }) => {
    const nick = uniqueNick();
    const ch1 = uniqueChannel();
    const ch2 = uniqueChannel();
    await connectGuest(page, nick, `${ch1}, ${ch2}`);

    const sidebar = page.getByTestId('sidebar');
    await expect(sidebar.getByText(ch2)).toBeVisible({ timeout: 10_000 });

    // Part from ch2
    await sendMessage(page, `/part ${ch2}`);

    // ch2 should disappear from sidebar
    await expect(sidebar.getByText(ch2)).not.toBeVisible({ timeout: 5_000 });

    // ch1 should still be there
    await expect(sidebar.getByText(ch1)).toBeVisible();
  });
});
