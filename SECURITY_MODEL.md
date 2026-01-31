# Security Model — Butterfly Bot (Tor + E2E)

This document explains how the system reduces security risks and where the boundaries are. No system can eliminate all risk; the goal is to **minimize attack surface, prevent silent compromise, and make failures visible**.

## 1) Threat Model (What We Protect Against)
- Network surveillance and metadata collection.
- MITM attacks during transport.
- Server compromise (central bot or relay compromise).
- Device loss or local storage leaks.
- Key substitution and impersonation.

## 2) Core Guarantees
- **E2E Encryption by default** for all messages.
- **Forward secrecy** and **post‑compromise recovery** via ratcheting (planned).
- **Tor transport** to hide IPs and reduce metadata leakage.
- **Private keys never stored in plaintext**, only in OS keyring.

## 3) Key Principles That Reduce Risk
### Identity + Device Separation
- **Identity Key (IK)** is long‑term and used only for authentication.
- **Per‑device keys** reduce blast radius if one device is compromised.
- **Key change warnings** prevent silent key substitution.

### Verification & Trust
- **Safety numbers** for human verification.
- **Trust states** (unverified/verified/blocked) enforce explicit consent.
- **Key change events** force re‑verification.

### Transport Security
- Tor provides network‑layer anonymity (no public IP exposure).
- Onion‑to‑onion traffic reduces reliance on exit nodes.

### Storage Security
- **Private keys**: OS keyring only (GNOME Keyring/Secret Service).
- **Local DB**: encrypted at rest (planned) + minimal stored metadata.

## 4) What the Bot Can and Cannot See
- Bot receives only **E2E‑decrypted content** if the user explicitly opts to interact with it.
- Bot **never stores plaintext** by default.
- Logs are local and optional.

## 5) Failure Modes and How We Prevent Silent Failure
- **Key replacement** → blocks sending until re‑verified.
- **Compromised device** → revoke device, rotate keys.
- **Transport failure** → messages fail closed (no downgrade to plaintext).

## 6) Explicit Non‑Goals
- We do not attempt to secure a fully compromised OS.
- We do not promise metadata‑free communication (Tor reduces it, does not eliminate it).

## 7) Operational Rules
- **No plaintext fallback.**
- **No storing secrets in SQLite.**
- **No key changes without user confirmation.**
- **No silent protocol downgrades.**
- **Device inactivity auto‑deactivation** to reduce silent‑spy risk.

## 8) Next Security Work (Planned)
- Double Ratchet session keys.
- MLS / group keying (if needed).
- Encrypted local DB (SQLCipher).
- Device‑pairing and key‑rotation UX.
- System‑wide **inactive device deactivation** policy + UX.

---
If you want this tightened into a formal “Security Guarantees + Limitations” spec, I’ll rewrite it as a formal policy doc.
