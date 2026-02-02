# Butterfly Bot — Privacy‑First P2P Chat (Tor + E2E) Plan

> **Purpose**: Capture what is already built vs. what remains to deliver a Tor‑routed, E2E‑encrypted, desktop‑only chat app with a **central bot**. No code changes here; this is the execution plan and status.

## 1) Current State (Confirmed in Repo)
Based on the current workspace and README:

- **Desktop app exists (Dioxus)** with streaming chat UI.  
- **Local bot runs on the user’s PC** (local‑first).  
- **SQLite** used for local storage.  
- **LanceDB** used for memory/embedding retrieval.  
- **Reminders + notifications** exist (chat + OS notifications).  
- **Tools + settings UI** exist (enable/disable tools, configure providers, memory).  
- **Secrets stored in OS keychain** (GNOME Keyring/Secret Service), not plain files.  
- **Rust codebase** with library + multiple binaries (CLI and UI).  

## 1.5) Start Here (tg Branch)
**Immediate objective**: prove Tor connectivity in isolation before touching UI or E2E.

### Week‑0 Tasks (Arti Spike)
1. Create a minimal Tor connectivity test (client only): connect to a known onion echo service and exchange a ping.
2. Add a thin **Transport** abstraction (Tor vs. local) without wiring into UI yet.
3. Document latency, reliability, and failure modes in this plan.
4. Decide the E2E protocol stack once transport is validated.

### Definition of Done (Arti Spike)
- Able to establish a Tor circuit and reach a test onion endpoint.
- Sends a ping and receives a response within an acceptable latency window.
- Logs connection lifecycle and failure cases (timeouts, retries, bootstrap issues).
- Produces a short write‑up added to this plan with measured latency and stability notes.

## 2) Current State (Likely/Needs Verification)
Items that appear implied by the repo but should be verified before implementation planning:

- Local daemon / background services may exist (scheduler, reminders, brain plugins).  
- Provider integrations include local and OpenAI‑compatible endpoints.  
- Tests cover memory, plugins, providers, scheduling.  

## 3) Target Product Definition
**Goal**: “Telegram‑like” user experience on desktop, but private by default.

- **Transport**: Tor onion services (no public IP exposure).  
- **E2E Encryption**: Always on for user↔user and user↔bot.  
- **Central Bot**: Hosted as an onion service; users chat with it as a peer.  
- **Desktop only**: Keep Dioxus app; remove CLI support.  
- **Local‑first**: Store conversations + memory locally (encrypted at rest).  
- **Future**: Solana automated trading integration (strict signing policies).  

## 4) Work Completed vs. Work Remaining

### ✅ Completed (Core App Foundations)
- Desktop UI with streaming responses.
- Local memory stack (SQLite + LanceDB).
- Settings UI with provider credentials and tool toggles.
- Local data storage and OS keychain secrets.
- Reminders + notifications (integrated into UI).

### 🟡 In Progress / Needs Verification
- Background scheduler and plugin systems (present in code, confirm UX integration).
- Bot orchestration / routing design (verify how bot logic is scoped).

### ❗ Not Started (Required for P2P + Tor + E2E)
**Transport & Network**
- Embed Tor client (Arti) in desktop app.
- Add onion‑service support for the central bot.
- Peer discovery model (exchange onion addresses securely).
- Message delivery protocol over Tor (retries, ordering, acknowledgements).
- Traffic obfuscation (padding, batching, optional delays).

**Cryptography (E2E)**
- Identity keys and device keys per user.
- X3DH/Noise‑style handshake + Double Ratchet.
- Forward secrecy + post‑compromise security.
- Per‑conversation key management and rotation.
- Encrypted attachments (if/when added).

**Storage & Privacy Hardening**
- Encrypted local database at rest (SQLCipher or equivalent).
- Key storage policies (OS keychain + local KMS abstraction).
- Message retention controls and auto‑delete policies.

**Bot Centralization**
- Central bot as onion service with per‑user E2E sessions.
- Ensure bot never stores plaintext by default.
- Add explicit user consent for model logs (off by default).

**UX & Product**
- Contact management (onion address book, trust labels, key fingerprints).
- Key verification UX (QR code / safety number).
- Device pairing and backup/recovery workflows.
- Notification behavior for offline/online presence.

**De‑CLI**
- Remove CLI binaries or mark them deprecated.
- Update build/run docs to reflect desktop‑only.

**Compliance & Safety**
- Clear privacy policy and security model in docs.
- Local‑only telemetry (opt‑in) if ever needed.

## 5) Architecture Plan (Desktop + Tor + E2E)

### 5.1 Transport
- **Tor only** using Arti.
- Central bot runs as an onion service; peers connect via onion addresses.
- Optional relays are not needed; Tor handles routing.

### 5.2 E2E Protocol
- **Handshake**: X3DH/Noise for session bootstrap.
- **Ratchet**: Double Ratchet for ongoing messages.
- **Group chat (later)**: Sender Keys or MLS.

### 5.3 Storage
- **Local encrypted store** (SQLite + SQLCipher).
- **Vector memory** via LanceDB remains local.
- **Keychain** for identity keys and secrets.

### 5.4 Bot Integration
- Bot runs local for personal use **or** central onion service for shared use.
- Each user has a distinct E2E session with bot.
- Bot responses are never stored plaintext unless user opts in.

## 5.5) Telegram‑Like UX/UI (Basics Only)
**Goal**: replicate Telegram’s familiar layout and interaction patterns, but only the core MVP features.

### MVP UX Features
- Left sidebar with **Chats list** (recency‑sorted, unread badges).
- Main pane with **message thread** (grouped bubbles, timestamps).
- **Top bar** with chat title, last seen/online indicator.
- **Composer** with send button, attachments stub (no file transfer yet).
- **Search** across chats (local only).
- **Settings** with identity keys, Tor status, memory toggles.

### MVP Scope (Explicitly Out‑of‑Scope)
- No channels, stories, or voice/video.
- No stickers, themes, or rich bots (beyond the central bot chat).
- No multi‑device sync v1 (single desktop device).

## 5.6) Identity Management (IDM) + Multi‑Device (Critical)
**Goal**: prevent identity compromise and key confusion across devices.

### Identity Model
- **User Identity Key (IK):** long‑term X25519/Ed25519 key pair.
- **Device Keys:** per‑device keys derived/registered under the user identity.
- **Trust States:** unverified → verified → blocked.
- **Safety Number/Fingerprint:** derived from both parties’ identity keys.

### Storage Rules
- **Private keys** stored only in OS keyring (never in SQLite).
- **Public keys + fingerprints** stored in SQLite for UX and verification history.
- **Key change events** are written to local audit log.

### Multi‑Device Strategy (v1)
- **Device list** per user identity (device_id, label, last_seen).
- **Device enrollment** requires explicit approval on an existing device.
- **Session keys** are per‑device to avoid cross‑device replay.
- **Key rotation** triggers re‑verification for all peer contacts.
- **Inactivity deactivation**: auto‑disable devices after a configurable idle period.

### UX Flows (Required)
1. **First‑run identity creation** (generate IK, show safety number).
2. **Add device** (QR code / pairing code, approve on existing device).
3. **Key change warning** (block sending until user re‑verifies).
4. **Lost device revoke** (remove device, invalidate sessions).
5. **Inactive device notice** (auto‑deactivate + user prompt to re‑enable).

### Risks if IDM is wrong
- Identity spoofing, MITM, or silent key replacement.
- Cross‑device state divergence causing message loss or disclosure.

## 5.7) Encrypted SQLite at Rest (SQLCipher)
**Goal**: encrypt *all* local data at rest, including non‑private metadata.

### Plan
- Replace SQLite with **SQLCipher** (libsqlite3‑sys with bundled SQLCipher).
- Store DB encryption key in OS keyring.
- On DB open: run `PRAGMA key = ?;` and `PRAGMA cipher_compatibility = 4;`.

### Notes
- Diesel can work with SQLCipher when linked against the SQLCipher SQLite library.
- This is required before multi‑device trust metadata is stored.

## 6) Execution Phases

### Phase 0 — Verification & Design (1–2 weeks)
- Audit repo for existing network/daemon behavior.
- Confirm current UI flows and state storage.
- Write crypto + Tor threat model.
- Decide protocol specifics (Noise patterns, ratchet implementation).

### Phase 1 — Tor Transport (2–4 weeks)
- Embed Arti into the desktop app.
- Create Tor session manager and onion client connectors.
- Add onion service for central bot.

### Phase 2 — E2E Protocol (3–6 weeks)
- Identity keys and key storage.
- X3DH/Noise handshake + Double Ratchet.
- Message envelopes + ack/retry.

### Phase 3 — UX & Contact Model (3–5 weeks)
- Contact onboarding and verification UI.
- Key fingerprint & safety number workflow.
- Presence/availability indicators (local only).

### Phase 4 — Hardening & Privacy (2–4 weeks)
- Encrypted DB at rest.
- Padding/traffic obfuscation settings.
- Default retention policy and secure wipe.

### Phase 5 — De‑CLI & Release Readiness (1–2 weeks)
- Remove CLI targets and docs.
- Update README for desktop‑only + Tor/E2E.
- Add security.md and privacy.md.

## 6.5) Decentralized Bot via Solana (Optional, Non‑Centralized)
**Goal**: avoid a single central bot endpoint by distributing bot providers while keeping privacy intact.

### Model
- **On‑chain registry (Solana)** publishes:
	- provider onion address/service endpoint
	- provider public identity key
	- pricing + model capabilities
	- uptime/reputation metadata
- **Off‑chain workers** run the bot compute and serve requests over Tor.
- **Client selection** chooses providers based on stake, reputation, latency, or price.
- **E2E stays off‑chain**; the chain never sees message content or keys.

### Incentives & Security
- **Staking**: providers lock stake to join the registry.
- **Reputation**: client‑signed ratings and uptime proofs.
- **Slashing**: penalties for fraud, downtime, or policy violations.
- **Payment**: Solana micropayments or pre‑paid credits.

### Privacy Boundaries
- Chain stores only metadata and proofs.
- Message traffic and keys are Tor‑routed and E2E encrypted.
- No plaintext logs by default; opt‑in only.

### Risks / Tradeoffs
- Sybil resistance requires careful staking design.
- On‑chain data is public; limit what is published.
- Latency may rise due to Tor + distributed selection.

### MVP for Decentralized Bot
1. On‑chain registry + provider identity keys.
2. Client UI for selecting providers (manual first).
3. Tor connectivity + E2E sessions to selected provider.
4. Payment + reputation v1.

## 6.6) $AGENT Token Utility (Jupiter Verified)
**Goal**: make $AGENT a core economic and governance primitive without leaking private data.

### Utility Design
- **Provider staking**: bot providers stake $AGENT to join the registry.
- **Reputation weighting**: stake size + uptime to rank providers.
- **Payment rail**: users pay providers in $AGENT for compute or sessions.
- **Slashing**: protocol‑level penalties for fraud, downtime, or policy violations.
- **Governance** (optional): $AGENT holders vote on protocol parameters.

### Privacy Constraints
- On‑chain only stores economic metadata; **never** message content or keys.
- Payments can be aggregated or pre‑paid to reduce linking.

### MVP Scope for $AGENT
1. Registry stake requirement in $AGENT.
2. Basic payment flow (pre‑paid credits or per‑session).
3. Provider ranking based on stake + uptime.

## 7) Risks & Open Questions
- **Protocol choice**: use Signal‑style vs. MLS for groups.
- **Tor UX**: startup latency and reliability in strict networks.
- **Key recovery**: safe recovery UX without weak security.
- **Bot privacy**: controlling model logs and token cost.
- **Regulatory**: ensure clear documentation of E2E boundaries.

## 8) Immediate Next Steps (Non‑Coding)
1. Approve this plan and confirm the target protocol stack.
2. Identify required UX screens and update the product spec.
3. Decide whether the central bot is hosted by you or end‑users.
4. Define minimal viable E2E feature set for v1.

## 9) MLS (OpenMLS) — Group E2E Strategy
**Decision**: MLS ships **after** 1:1 P2P E2E is proven stable; keep 1:1 + bot first.

### Why MLS
- Standardized group E2E with efficient member changes.
- Forward secrecy and post‑compromise security for groups.

### Integration Plan (OpenMLS)
1. Add OpenMLS crate and implement group key store (keyring for private, SQLite for public state).
2. Define group membership service (invite, add/remove, epoch updates).
3. Add MLS message envelopes and routing in transport layer.
4. Build UX for membership changes + verification prompts.

### Risks
- More complex state management and recovery workflows.
- Needs careful UX to avoid silent key‑change failures.

---
If you want, I can now tailor this plan to your exact UX flows and produce a visual architecture diagram in markdown.
