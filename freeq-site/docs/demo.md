# Policy Framework Demo

Try freeq's channel governance live on **irc.freeq.at** — no install needed.

## What you'll do

1. Create a policy-gated channel
2. Add a Bluesky follower requirement
3. Watch a second user get blocked, follow you, and gain access

Total time: ~5 minutes.

---

## Step 1: Sign in

Go to [irc.freeq.at](https://irc.freeq.at) and sign in with your Bluesky account.

You'll get your Bluesky handle as your IRC nick (e.g. `chadfowler.com`).

## Step 2: Create a channel with a policy

Join a new channel — you'll automatically become operator:

```
/join #my-demo
```

Set a base policy (accept-rules):

```
/policy #my-demo set accept-rules
```

Now add a Bluesky follower requirement. Replace `yourhandle` with your actual Bluesky handle:

```
/policy #my-demo require bluesky_follower issuer=did:web:irc.freeq.at:verify url=/verify/bluesky/start?target=yourhandle.bsky.social label=Follow_@yourhandle
```

Check the policy is set:

```
/policy #my-demo info
```

## Step 3: Test with a second user

Open an **incognito/private window** and go to [irc.freeq.at](https://irc.freeq.at). Sign in with a different Bluesky account.

Try to join the channel:

```
/join #my-demo
```

You'll see a **JoinGateModal** with two requirements:

- ☐ Accept the channel rules
- ☐ Follow @yourhandle on Bluesky (🦋 button)

### If the second account doesn't follow you:

1. Click the 🦋 **Follow @yourhandle** button
2. A popup shows: "Follow Required" with a link to your Bluesky profile
3. Open the link, follow the account on Bluesky
4. Click **"I followed — check again"**
5. The popup confirms: "✓ Verified"
6. Back in the modal, click **Join Channel**

The user joins `#my-demo` with `member` role.

### If the second account already follows you:

1. Click the 🦋 button — instant verification (no follow needed)
2. Click **Join Channel**

## Step 4: Try the Channel Settings panel

Back in the first window (as channel op), click the ⚙️ gear icon in the top bar.

The **Channel Settings** panel has three tabs:

- **Rules**: See the current policy, update or clear it
- **Verifiers**: Add/remove credential verifiers (GitHub, Bluesky)
- **Roles**: Set auto-op or auto-voice based on credentials

Try adding a second verifier from the Verifiers tab — the dropdown has presets for GitHub repos, GitHub orgs, and Bluesky followers.

## Step 5: Clean up

Clear the policy to make the channel open again:

```
/policy #my-demo clear
```

---

## What just happened?

Under the hood:

1. `POLICY SET accept-rules` created a policy document with a cryptographic rules hash
2. `POLICY REQUIRE` added a `PRESENT` requirement for `bluesky_follower` credentials AND a credential endpoint (UI metadata)
3. When the second user tried to JOIN, the server checked for a valid membership attestation — none existed, so it returned 477
4. The web client's JoinGateModal called `POST /api/v1/policy/#my-demo/check` with the user's DID
5. The check endpoint evaluated requirements and returned which were satisfied
6. The 🦋 button opened `/verify/bluesky/start` — the verifier checked the public Bluesky API (`app.bsky.graph.getFollows`) for the follow relationship
7. The verifier signed a `FreeqCredential/v1` with Ed25519 and POSTed it to the server's `/api/v1/credentials/present`
8. When the user clicked "Join Channel", the client sent `POLICY #my-demo ACCEPT` (creating the membership attestation) then `JOIN #my-demo`
9. The server found the valid attestation and allowed the join

**Zero API keys were needed.** The Bluesky follower check uses the public social graph. The credential is signed by the verifier's Ed25519 key and verified by resolving the verifier's DID document.

---

## Other policy types

### GitHub org membership

Requires `GITHUB_CLIENT_ID` and `GITHUB_CLIENT_SECRET` env vars on the server.

```
/policy #team set accept-rules
/policy #team require github_membership issuer=did:web:irc.freeq.at:verify url=/verify/github/start?org=myorg label=Verify_GitHub
```

### GitHub repo collaborator with auto-op

```
/policy #project set accept-rules
/policy #project set-role op {"type":"PRESENT","credential_type":"github_repo","issuer":"did:web:irc.freeq.at:verify"}
/policy #project require github_repo issuer=did:web:irc.freeq.at:verify url=/verify/github/start?repo=owner/repo label=Verify_Repo
```

Users with push access to the repo automatically get operator (+o) on join.

---

## Learn more

- [Policy Framework architecture](/docs/policy-framework) — requirement DSL, attestations, transparency log
- [Verifiers](/docs/verifiers) — how to build custom verifiers
- [API Reference](/docs/api-reference) — REST endpoints for policy management
