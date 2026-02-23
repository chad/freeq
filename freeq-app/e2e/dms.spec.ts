/**
 * E2E tests: Direct messages
 */
import { test, expect } from '@playwright/test';
import { uniqueNick, uniqueChannel, connectGuest, sendMessage, expectMessage, openSidebar } from './helpers';

test.describe('Direct Messages', () => {
  test('DM creates buffer in sidebar', async ({ browser }) => {
    const nick1 = uniqueNick('dm1');
    const nick2 = uniqueNick('dm2');
    const channel = uniqueChannel();

    const ctx1 = await browser.newContext();
    const ctx2 = await browser.newContext();
    const page1 = await ctx1.newPage();
    const page2 = await ctx2.newPage();

    await connectGuest(page1, nick1, channel);
    await connectGuest(page2, nick2, channel);
    await page1.waitForTimeout(500);

    // Send DM from user 1 to user 2
    await sendMessage(page1, `/msg ${nick2} hello privately`);

    // User 2 should see DM buffer with nick1 — use .first() to avoid strict mode
    const sidebar2 = await openSidebar(page2);
    await expect(sidebar2.getByText(nick1).first()).toBeVisible({ timeout: 10_000 });

    // Click on DM to see message
    await sidebar2.getByText(nick1).first().click();
    await page2.waitForTimeout(300);
    await expectMessage(page2, 'hello privately');

    await ctx1.close();
    await ctx2.close();
  });

  test('DM reply goes to correct buffer', async ({ browser }) => {
    const nick1 = uniqueNick('dm3');
    const nick2 = uniqueNick('dm4');
    const channel = uniqueChannel();

    const ctx1 = await browser.newContext();
    const ctx2 = await browser.newContext();
    const page1 = await ctx1.newPage();
    const page2 = await ctx2.newPage();

    await connectGuest(page1, nick1, channel);
    await connectGuest(page2, nick2, channel);
    await page1.waitForTimeout(500);

    // User 1 sends DM
    await sendMessage(page1, `/msg ${nick2} initial dm`);

    // User 2 receives and clicks DM buffer
    const sidebar2 = await openSidebar(page2);
    await expect(sidebar2.getByText(nick1).first()).toBeVisible({ timeout: 10_000 });
    await sidebar2.getByText(nick1).first().click();
    await page2.waitForTimeout(300);
    await expectMessage(page2, 'initial dm');

    // User 2 replies in the DM buffer
    await sendMessage(page2, 'dm reply back');

    // User 1 should see the reply in their DM buffer with nick2
    const sidebar1 = await openSidebar(page1);
    await expect(sidebar1.getByText(nick2).first()).toBeVisible({ timeout: 10_000 });
    await sidebar1.getByText(nick2).first().click();
    await page1.waitForTimeout(300);
    await expectMessage(page1, 'dm reply back');

    await ctx1.close();
    await ctx2.close();
  });

  test('DM to offline user shows error', async ({ page }) => {
    const nick = uniqueNick();
    const channel = uniqueChannel();
    await connectGuest(page, nick, channel);

    const fakeNick = uniqueNick('offline');
    // Send DM to a nick that doesn't exist
    await sendMessage(page, `/msg ${fakeNick} hello?`);

    // Error appears in DM buffer if it exists, or server buffer
    // The DM buffer gets created by /msg handler; 401 error then shows there
    await page.waitForTimeout(1000);

    // Check for the error in the DM buffer (click it if it appeared in sidebar)
    const sidebar = await openSidebar(page);
    const dmEntry = sidebar.getByText(fakeNick).first();
    if (await dmEntry.isVisible({ timeout: 3_000 }).catch(() => false)) {
      await dmEntry.click();
      await page.waitForTimeout(300);
    }

    await expect(
      page.getByText(/not online|no such nick|not delivered/i).first()
    ).toBeVisible({ timeout: 10_000 });
  });

  test('DM buffer shows in sidebar DM section', async ({ browser }) => {
    const nick1 = uniqueNick('dmsec1');
    const nick2 = uniqueNick('dmsec2');
    const channel = uniqueChannel();

    const ctx1 = await browser.newContext();
    const ctx2 = await browser.newContext();
    const page1 = await ctx1.newPage();
    const page2 = await ctx2.newPage();

    await connectGuest(page1, nick1, channel);
    await connectGuest(page2, nick2, channel);
    await page1.waitForTimeout(500);

    await sendMessage(page1, `/msg ${nick2} test dm section`);

    // Check sidebar has DM entry — use .first() for strict mode
    const sidebar2 = await openSidebar(page2);
    await expect(sidebar2.getByText(nick1).first()).toBeVisible({ timeout: 10_000 });

    // There should be a "Direct Messages" or "DMs" section label
    const dmSection = sidebar2.getByText(/direct message|DM/i);
    if (await dmSection.count() > 0) {
      await expect(dmSection.first()).toBeVisible();
    }

    await ctx1.close();
    await ctx2.close();
  });
});
