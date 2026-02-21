/**
 * E2E tests: Messaging between users
 */
import { test, expect } from '@playwright/test';
import { uniqueNick, uniqueChannel, connectGuest, sendMessage, expectMessage } from './helpers';

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

    // Wait briefly for member sync
    await page1.waitForTimeout(500);

    // Alice sends a message
    await sendMessage(page1, `hello from ${nick1}`);

    // Bob should see it
    await expectMessage(page2, `hello from ${nick1}`);

    // Bob replies
    await sendMessage(page2, `hey ${nick1}, bob here`);

    // Alice should see it
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
});
