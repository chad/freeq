/**
 * E2E tests: Messaging between users
 */
import { test, expect } from '@playwright/test';
import { uniqueNick, uniqueChannel, connectGuest, sendMessage, expectMessage, connectSecondUser, openSidebar, switchChannel } from './helpers';

test.describe('Messaging', () => {
  test('user can send and see own message', async ({ page }) => {
    const nick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    await sendMessage(page, 'hello from playwright');
    await expectMessage(page, 'hello from playwright');
  });

  test('two users can chat in same channel', async ({ browser }) => {
    const nick1 = uniqueNick('alice');
    const nick2 = uniqueNick('bob');
    const channel = uniqueChannel();

    const ctx1 = await browser.newContext();
    const ctx2 = await browser.newContext();
    const page1 = await ctx1.newPage();
    const page2 = await ctx2.newPage();

    await connectGuest(page1, nick1, channel);
    await connectGuest(page2, nick2, channel);
    await page1.waitForTimeout(500);

    await sendMessage(page1, `hello from ${nick1}`);
    await expectMessage(page2, `hello from ${nick1}`);

    await sendMessage(page2, `hey ${nick1}, bob here`);
    await expectMessage(page1, `hey ${nick1}, bob here`);

    await ctx1.close();
    await ctx2.close();
  });

  test('message appears with sender nick', async ({ page }) => {
    const nick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    await sendMessage(page, 'test message with nick');

    const msgList = page.getByTestId('message-list');
    await expect(msgList.getByText(nick).first()).toBeVisible();
    await expect(msgList.getByText('test message with nick')).toBeVisible();
  });

  test('messages have timestamps', async ({ page }) => {
    const nick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    await sendMessage(page, 'timestamp test');
    await expectMessage(page, 'timestamp test');

    // Timestamps use whitespace-nowrap class
    const timeEl = page.getByTestId('message-list').locator('[class*="whitespace-nowrap"]').first();
    await expect(timeEl).toBeVisible({ timeout: 5_000 });
  });

  test('/me action message renders differently', async ({ page }) => {
    const nick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    await sendMessage(page, '/me waves hello');
    await expect(page.getByText('waves hello')).toBeVisible({ timeout: 5_000 });
  });

  test('messages persist across channel switches', async ({ page }) => {
    const nick = uniqueNick();
    const ch1 = uniqueChannel();
    const ch2 = uniqueChannel();
    await connectGuest(page, nick, `${ch1}, ${ch2}`);

    // Explicitly switch to ch1 first (active channel after connect may vary)
    await switchChannel(page, ch1);
    await sendMessage(page, 'persistent message');
    await expectMessage(page, 'persistent message');

    // Switch to ch2
    await switchChannel(page, ch2);
    await sendMessage(page, 'ch2 message');
    await expectMessage(page, 'ch2 message');

    // Switch back to ch1 — message should still be there
    await switchChannel(page, ch1);
    await expectMessage(page, 'persistent message');
  });

  test('no duplicate messages on channel switch', async ({ page }) => {
    const nick = uniqueNick();
    const ch1 = uniqueChannel();
    const ch2 = uniqueChannel();
    await connectGuest(page, nick, `${ch1}, ${ch2}`);

    // Explicitly switch to ch1 first
    await switchChannel(page, ch1);
    await sendMessage(page, 'unique msg 12345');
    await expectMessage(page, 'unique msg 12345');

    // Switch away and back
    await switchChannel(page, ch2);
    await page.waitForTimeout(500);
    await switchChannel(page, ch1);
    await page.waitForTimeout(1000);

    // Count occurrences of the message — should be exactly 1
    const count = await page.getByTestId('message-list').getByText('unique msg 12345').count();
    expect(count).toBe(1);
  });

  test('long message wraps properly', async ({ page }) => {
    const nick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    const longMsg = 'A'.repeat(300);
    await sendMessage(page, longMsg);

    await expectMessage(page, longMsg.slice(0, 50));
    const bodyWidth = await page.evaluate(() => document.body.scrollWidth);
    const vpWidth = page.viewportSize()!.width;
    expect(bodyWidth).toBeLessThanOrEqual(vpWidth + 5);
  });

  test('URL in message is rendered as link', async ({ page }) => {
    const nick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    await sendMessage(page, 'check out https://example.com/test');
    await expectMessage(page, 'check out');

    const link = page.getByTestId('message-list').locator('a[href="https://example.com/test"]');
    await expect(link).toBeVisible({ timeout: 5_000 });
  });

  test('DM can be sent via /msg command', async ({ browser }) => {
    const nick1 = uniqueNick('sender');
    const nick2 = uniqueNick('rcvr');
    const channel = uniqueChannel();

    const ctx1 = await browser.newContext();
    const ctx2 = await browser.newContext();
    const page1 = await ctx1.newPage();
    const page2 = await ctx2.newPage();

    await connectGuest(page1, nick1, channel);
    await connectGuest(page2, nick2, channel);
    await page1.waitForTimeout(500);

    // Send DM from user 1 to user 2
    await sendMessage(page1, `/msg ${nick2} hey private msg`);

    // User 2 should see a DM buffer appear in sidebar — use .first() for strict mode
    const sidebar2 = await openSidebar(page2);
    await expect(sidebar2.getByText(nick1).first()).toBeVisible({ timeout: 10_000 });

    // Click on the DM buffer to see the message
    await sidebar2.getByText(nick1).first().click();
    await page2.waitForTimeout(300);
    await expectMessage(page2, 'hey private msg');

    await ctx1.close();
    await ctx2.close();
  });

  test('nick change command works', async ({ page }) => {
    const oldNick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, oldNick, channel);

    const newNick = uniqueNick('newnick');
    await sendMessage(page, `/nick ${newNick}`);

    await expect(page.getByText(newNick).first()).toBeVisible({ timeout: 5_000 });
  });
});
