# App Store Deployment Setup

Status: 2026-06-19

This is the setup checklist for shipping freeq on the iOS App Store and Mac App Store.

## Current repo state

- Local branch is `main`, currently 6 commits ahead and 6 commits behind `origin/main`.
- iOS is generated from `freeq-ios/project.yml`; `xcodegen` is installed locally.
- iOS main app currently uses `at.freeq.ios`, version `1.0.0`, build `1`, development team `3DT7XF7L4R`.
- iOS extension IDs exist in config but are not embedded in the main app target yet:
  - `at.freeq.ios.liveactivity`
  - `at.freeq.ios.watchkitapp`
- macOS currently uses `at.freeq.macos`, version `1.0.0`, build `1`.
- macOS does not currently set `DEVELOPMENT_TEAM` in the checked-in project.
- macOS App Sandbox is currently disabled (`com.apple.security.app-sandbox = false`), which must be fixed for Mac App Store submission.
- Neither iOS nor macOS currently has a `PrivacyInfo.xcprivacy` privacy manifest checked in.

## Setup order

- [x] Inspect local project settings and signing metadata.
- [x] Check current Apple requirements against Apple documentation.
- [ ] Settle the release source of truth: pull/rebase/merge so the release branch is not both ahead and behind remote.
- [ ] Verify `freeq.at` email can receive mail before Apple Developer Program organization enrollment.
- [ ] Decide App Store product model before creating App Store Connect records or uploading builds.
- [ ] Create public web pages required for review: privacy policy, support/contact, account/data deletion instructions.
- [ ] Register Apple bundle IDs and capabilities.
- [ ] Fix macOS sandbox/signing settings.
- [ ] Add privacy manifests and privacy-label inventory.
- [ ] Archive iOS and upload to TestFlight.
- [ ] Archive macOS and upload to TestFlight.
- [ ] Prepare screenshots, metadata, demo/review account, and App Review notes.
- [ ] Submit first iOS and macOS builds for review.

## Apple account and domain prerequisites

For an organization account, Apple requires a legal entity, a D-U-N-S number, and a work email address associated with the organization's domain. Use a real deliverable mailbox on `freeq.at`, such as:

- `developer@freeq.at` for Apple Developer enrollment and account ownership.
- `support@freeq.at` for App Store support URL/contact.
- `privacy@freeq.at` for privacy/data requests.

The domain email does not need to be complicated, but it does need to reliably receive Apple verification and review mail. Finish MX/SPF/DKIM/DMARC enough that Apple mail is not lost or quarantined.

Also make sure the Account Holder signs current agreements in App Store Connect before trying to create app records or submit builds.

## Product-record decision

Decide this before the first App Store Connect app record or first uploaded build.

### Option A: one universal `freeq` listing

Best user experience if the iOS and macOS apps should be one product page and one App Store name.

Required repo change before first upload:

- Change the iOS main app and macOS app to the same canonical bundle ID, for example `at.freeq.app`.
- Update related extension IDs if enabled:
  - `at.freeq.app.liveactivity`
  - `at.freeq.app.watchkitapp`
- Register the same main bundle ID for both iOS and macOS in Apple Developer/App Store Connect.

This avoids App Store name collisions and gives one product page for `freeq`.

### Option B: separate iOS and macOS listings

Least repo churn because it matches current bundle IDs:

- iOS: `at.freeq.ios`
- macOS: `at.freeq.macos`

Tradeoff: the App Store name may need to differ per record/localization, such as `freeq` and `freeq for Mac`, and users see separate product pages.

## App IDs and capabilities

Register explicit App IDs in Apple Developer Certificates, Identifiers & Profiles. Start with the main app only, then add extensions when they are actually embedded and release-ready.

Recommended first TestFlight scope:

- iOS main app only.
- macOS app only.
- Leave watch and live activity disabled until their provisioning, entitlements, and review story are clean.

iOS capabilities to verify:

- Custom URL scheme `freeq` is in Info.plist.
- Camera, microphone, photo library, and speech usage strings are present.
- Add push notification capability only if remote notifications/APNs are actually used.
- Add Live Activities only if `freeqLiveActivity` is embedded and release-ready.
- Add Watch target only if the watch app is embedded and release-ready.

macOS capabilities to verify:

- Enable App Sandbox.
- Keep network client entitlement.
- Add sandbox device entitlements for camera, microphone, and speech if those features ship in the Mac App Store build.
- Confirm all embedded frameworks/binaries are signed and sandbox-compatible.

## Privacy, review, and legal blockers

Before upload:

- Add `PrivacyInfo.xcprivacy` to iOS and macOS targets.
- Inventory collected data for App Store privacy labels:
  - account identifiers / DID / handle
  - user-generated messages and uploads
  - contacts/social graph if Bluesky profile data is fetched
  - diagnostics/logs if collected
  - audio/video/photo access
- Publish `https://freeq.at/privacy`.
- Publish a support page or mailto-backed page, for example `https://freeq.at/support`.
- Publish account/data deletion instructions, for example `https://freeq.at/delete-account`.
- Add in-app links to privacy/support/deletion in Settings.
- Prepare App Review notes explaining AT Protocol/Bluesky sign-in and guest mode.
- Create a reviewer-accessible demo identity, unless guest mode fully exercises the submitted feature set.

Important review concerns:

- If freeq is treated as using a third-party or social login for the primary account, expect Apple to scrutinize login-service compliance.
- If the app supports account creation, App Review expects an in-app account deletion path.
- If the app requires sign-in for important features, App Review needs working demo credentials or clear instructions.
- Because freeq uses encryption beyond plain OS HTTPS in parts of the stack, answer App Store export-compliance questions carefully before review.

## Build and upload commands

Regenerate iOS project after `project.yml` changes:

```bash
cd /Users/chad/src/freeq/freeq-ios
xcodegen generate
```

Archive iOS:

```bash
xcodebuild \
  -project /Users/chad/src/freeq/freeq-ios/freeq.xcodeproj \
  -scheme freeq \
  -configuration Release \
  -destination 'generic/platform=iOS' \
  -archivePath /tmp/freeq-ios.xcarchive \
  archive
```

Archive macOS:

```bash
xcodebuild \
  -project /Users/chad/src/freeq/freeq-macos/freeq-macos.xcodeproj \
  -scheme freeq-macos \
  -configuration Release \
  -destination 'generic/platform=macOS' \
  -archivePath /tmp/freeq-macos.xcarchive \
  archive
```

The first upload should be done from Xcode Organizer or Transporter after signing is configured and App Store Connect records exist.

## Apple references

- Apple Developer Program enrollment: https://developer.apple.com/programs/enroll/
- D-U-N-S requirement: https://developer.apple.com/help/account/membership/D-U-N-S/
- Register App IDs: https://developer.apple.com/help/account/identifiers/register-an-app-id/
- Enable app capabilities: https://developer.apple.com/help/account/identifiers/enable-app-capabilities/
- Add a new app record: https://developer.apple.com/help/app-store-connect/create-an-app-record/add-a-new-app/
- Add platforms / universal purchase: https://developer.apple.com/help/app-store-connect/create-an-app-record/add-platforms/
- Upload builds: https://developer.apple.com/help/app-store-connect/manage-builds/upload-builds/
- App privacy: https://developer.apple.com/help/app-store-connect/manage-app-information/manage-app-privacy/
- Privacy manifests: https://developer.apple.com/documentation/bundleresources/privacy-manifest-files
- Export compliance: https://developer.apple.com/help/app-store-connect/manage-app-information/overview-of-export-compliance/
- Mac App Sandbox: https://developer.apple.com/documentation/xcode/configuring-the-macos-app-sandbox
- App Review: https://developer.apple.com/distribute/app-review/
