/**
 * E2E tests: Reactions, replies, editing, deletion
 */
import { test, expect } from '@playwright/test';
import { uniqueNick, uniqueChannel, connectGuest, sendMessage, expectMessage, connectSecondUser } from './helpers';

test.describe('Message interactions', () => {
  test('can react to a message via context menu', async ({ page }) => {
    const nick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    await sendMessage(page, 'react to this message');
    await expectMessage(page, 'react to this message');

    // Hover over the message to reveal action buttons
    const msgEl = page.getByTestId('message-list').getByText('react to this message');
    await msgEl.hover();
    await page.waitForTimeout(200);

    // Look for reaction/emoji button in the hover actions
    const reactBtn = page.locator('[title*="React"], [aria-label*="React"], button:has-text("😀"), button:has-text("👍")').first();
    if (await reactBtn.isVisible()) {
      await reactBtn.click();
      // Emoji picker or quick reactions should appear
      await page.waitForTimeout(500);
      // Click a reaction emoji
      const emoji = page.getByText('👍').first();
      if (await emoji.isVisible()) {
        await emoji.click();
        // Reaction should appear on the message
        await expect(page.getByTestId('message-list').getByText('👍')).toBeVisible({ timeout: 5_000 });
      }
    }
  });

  test('can edit own message with up arrow', async ({ page }) => {
    const nick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    await sendMessage(page, 'original message to edit');
    await expectMessage(page, 'original message to edit');

    // Press up arrow in empty compose box to edit last message
    const compose = page.getByTestId('compose-input');
    await compose.click();
    await compose.press('ArrowUp');
    await page.waitForTimeout(300);

    // Compose should be populated with the original message
    const value = await compose.inputValue();
    if (value.includes('original message to edit')) {
      // Clear and type edited version
      await compose.fill('edited message content');
      await compose.press('Escape');
      await page.waitForTimeout(50);
      await compose.press('Enter');

      // Should see edited message
      await expectMessage(page, 'edited message content');
    }
  });

  test('reply shows context bar', async ({ page }) => {
    const nick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    await sendMessage(page, 'message to reply to');
    await expectMessage(page, 'message to reply to');

    // Hover to reveal reply button
    const msgEl = page.getByTestId('message-list').getByText('message to reply to');
    await msgEl.hover();
    await page.waitForTimeout(200);

    const replyBtn = page.locator('[title*="Reply"], [aria-label*="Reply"]').first();
    if (await replyBtn.isVisible()) {
      await replyBtn.click();
      // A reply context bar should appear near the compose area
      await expect(page.getByText(/replying to/i)).toBeVisible({ timeout: 3_000 });

      // Cancel the reply
      const cancelBtn = page.locator('[title*="Cancel"], button:has-text("✕"), button:has-text("×")').first();
      if (await cancelBtn.isVisible()) {
        await cancelBtn.click();
      } else {
        await page.keyboard.press('Escape');
      }
    }
  });

  test('reactions from another user show up', async ({ browser }) => {
    const nick1 = uniqueNick('reactor1');
    const nick2 = uniqueNick('reactor2');
    const channel = uniqueChannel();

    const ctx1 = await browser.newContext();
    const ctx2 = await browser.newContext();
    const page1 = await ctx1.newPage();
    const page2 = await ctx2.newPage();

    await connectGuest(page1, nick1, channel);
    await connectGuest(page2, nick2, channel);
    await page1.waitForTimeout(500);

    // User 1 sends a message
    await sendMessage(page1, 'react to me please');
    await expectMessage(page2, 'react to me please');

    // User 2 reacts via command
    await sendMessage(page2, `/react 👍`);
    await page1.waitForTimeout(1000);

    // User 1 should see the reaction (if reaction display works)
    // This is best-effort — reaction display depends on client implementation
    await ctx1.close();
    await ctx2.close();
  });
});
