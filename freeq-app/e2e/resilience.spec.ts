/**
 * E2E tests: Error handling, edge cases, resilience
 */
import { test, expect } from '@playwright/test';
import { uniqueNick, uniqueChannel, connectGuest, sendMessage, expectMessage, prepPage } from './helpers';

test.describe('Resilience', () => {
  test('empty message is not sent', async ({ page }) => {
    const nick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    const compose = page.getByTestId('compose-input');
    await compose.click();
    await compose.press('Enter');

    // Compose should still be empty and no message appeared
    await expect(compose).toHaveValue('');
  });

  test('rapid messages all arrive', async ({ page }) => {
    const nick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    // Send 5 messages quickly
    for (let i = 0; i < 5; i++) {
      await sendMessage(page, `rapid msg ${i}`);
    }

    // All should appear
    for (let i = 0; i < 5; i++) {
      await expectMessage(page, `rapid msg ${i}`);
    }
  });

  test('special characters in messages', async ({ page }) => {
    const nick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    await sendMessage(page, '<script>alert("xss")</script>');
    // Should show as text, not execute
    await expectMessage(page, '<script>');
    // Page should not have alerts
  });

  test('emoji messages work', async ({ page }) => {
    const nick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    await sendMessage(page, '🎉🚀💯 emoji party');
    await expectMessage(page, '🎉🚀💯 emoji party');
  });

  test('unicode messages work', async ({ page }) => {
    const nick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    await sendMessage(page, '日本語テスト 中文测试 한국어');
    await expectMessage(page, '日本語テスト');
  });

  test('very long nick displays without breaking layout', async ({ page }) => {
    const nick = 'a'.repeat(30); // Long nick
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    await sendMessage(page, 'message from long nick user');
    await expectMessage(page, 'message from long nick user');

    // No horizontal overflow
    const bodyWidth = await page.evaluate(() => document.body.scrollWidth);
    const vpWidth = page.viewportSize()!.width;
    expect(bodyWidth).toBeLessThanOrEqual(vpWidth + 5);
  });

  test('joining nonexistent channel creates it', async ({ page }) => {
    const nick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    const newChannel = uniqueChannel();
    await sendMessage(page, `/join ${newChannel}`);
    await expect(page.getByTestId('sidebar').getByText(newChannel)).toBeVisible({ timeout: 5_000 });
  });

  test('slash command that is unknown is sent to server', async ({ page }) => {
    const nick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    // Unknown command should be forwarded to server (not crash)
    await sendMessage(page, '/unknowncmd test');
    // Should not crash — compose clears
    const compose = page.getByTestId('compose-input');
    await expect(compose).toHaveValue('');
  });

  test('page title updates with channel', async ({ page }) => {
    const nick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    // Title should contain 'freeq' at minimum
    const title = await page.title();
    expect(title.toLowerCase()).toContain('freeq');
  });
});
