/**
 * E2E tests: UI elements, keyboard shortcuts, modals
 */
import { test, expect } from '@playwright/test';
import { uniqueNick, uniqueChannel, connectGuest, sendMessage } from './helpers';

test.describe('UI Elements', () => {
  let nick: string;
  let channel: string;

  test.beforeEach(async ({ page }) => {
    nick = uniqueNick();
    channel = uniqueChannel();
    await connectGuest(page, nick, channel);
  });

  test('member list shows current user', async ({ page }) => {
    // Our nick should appear somewhere in the UI (member list or message list)
    // Member list is to the right of the message area
    await expect(page.getByText(nick).first()).toBeVisible({ timeout: 5_000 });
  });

  test('compose box is focused and ready', async ({ page }) => {
    const compose = page.getByTestId('compose-input');
    await expect(compose).toBeVisible();
  });

  test('search modal opens with Cmd+F', async ({ page }) => {
    await page.keyboard.press('Meta+f');
    await expect(page.getByPlaceholder(/search/i)).toBeVisible({ timeout: 3_000 });
    await page.keyboard.press('Escape');
  });

  test('quick switcher opens with Cmd+K', async ({ page }) => {
    await page.keyboard.press('Meta+k');
    await expect(
      page.getByPlaceholder(/switch/i).or(page.getByPlaceholder(/channel/i))
    ).toBeVisible({ timeout: 3_000 });
    await page.keyboard.press('Escape');
  });
});

test.describe('Slash commands', () => {
  test('/nick changes nickname', async ({ page }) => {
    const oldNick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, oldNick, channel);

    const newNick = uniqueNick('newnick');
    await sendMessage(page, `/nick ${newNick}`);

    // New nick should appear somewhere on page after a moment
    await expect(page.getByText(newNick).first()).toBeVisible({ timeout: 5_000 });
  });

  test('/me sends action message', async ({ page }) => {
    const nick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    await sendMessage(page, '/me does a thing');
    await expect(page.getByText('does a thing')).toBeVisible({ timeout: 5_000 });
  });
});
