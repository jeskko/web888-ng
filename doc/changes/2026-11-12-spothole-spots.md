# Spothole spots, receive modes and activity filters

Date: 2026-11-12
Status: Implemented
Branch: jouni/spothole-spots, based on upstream main 091143b.

## Requested outcome
Read spots from spothole.app, reuse the existing FT8/WSPR spot list and waterfall label presentation, support USB/LSB/CW reception, and filter by Activity (WWFF, SOTA, POTA and other reported programmes). Deliver an independently reviewable feature branch suitable for an upstream PR. Existing locally decoded spots remain usable.

## Current behavior
- `ui/src/ctl/spots.tsx` has one list with band/mode/text filters; rows tune through `pickSpot`.
- `pickSpot` always selects USB and the local skimmer's dial. That is correct for the current FT8/FT4/WSPR data, but wrong for an external CW or voice spot.
- Local decoded spots come from the skimmer SpotBus, which also feeds outbound reporters. The web API/WebSocket exposes them, and `ui/src/view/pane.tsx` uses them for labels.
- The Spots button currently depends on local skimmers being available. Users must be able to use external spots even when none are configured.
- Local labels expire after mode-specific short windows; external DX activity requires a longer bounded lifetime.

## Research and verified external contract
Checked 2026-11-12:
- https://spothole.app/help/usage/clients documents public v3 JSON GET endpoints, recommended low request rates and an identifiable User-Agent/referrer. No key is required for reading spots. Posting is a separate authenticated operation and is outside scope.
- https://spothole.app/static/apidocs/openapi.yml specifies `/api/v3/spots`, a JSON array. Frequencies are Hz; `time` and `received_time` are Unix seconds; `activities` is an array, and `activity_refs` may contain several programme/reference pairs. A dual POTA/WWFF activation must match either activity.
- Spothole removed the USB/LSB distinction: both are SSB. CW is distinct. Sideband selection cannot be claimed to be provided by this feed.
- A single public request with an identifying User-Agent and `limit=3` confirmed array response/field types. The downloaded OpenAPI and sample remain in local scratch research, not copied wholesale into the PR.
- API terms favour one combined request plus local filtering, polling every few minutes, or SSE for genuinely immediate events. The proposed first implementation uses bounded snapshots, not SSE.

## Proposed behavior (recommended option A)
### Acquisition and privacy
Add an opt-in `[spots.spothole]` configuration section: enabled=false by default; HTTPS base URL defaults to https://spothole.app. The receiver makes one shared request every 60 seconds while enabled, irrespective of listener count. No listener browser connects to Spothole. The station name falls back to the existing kiwi.name, then station.callsign, when no explicit feed override is set. A public station URL has no existing config field and must be explicitly supplied; never infer it from a LAN listener URL. The owner can use another compatible self-hosted HTTPS instance. Disabling stops new requests and clears this source's cache; changing URL clears old-source data. Use an identifiable `web888d/VERSION` User-Agent with the upstream project URL and the explicitly configured public station name and URL. Send that public station URL as Referer; never infer it from a listener request. This station identity requirement was added by the owner after approval.

Fetch `/api/v3/spots` with a 30-minute window, limit=500 and an explicit field list. Only keep supported receive modes (CW, SSB/USB/LSB, FT8/FT4/WSPR; AM/FM may use existing demodulators if unambiguous). Do not silently map unsupported digital modes to USB. Retain original reported mode as metadata. Exclude QRT, malformed, implausibly future-dated, expired or out-of-receiver-range records. Cap response bytes at 2 MiB, requests at a 10-second timeout, cache at 500 validated entries. Use a Rust HTTPS client with rustls, without native OpenSSL, preserving static armv7 builds. Disable redirects rather than forwarding requests to an unexpected host.

On errors, retain only still-unexpired cached spots and show last successful update plus a clear unavailable/stale status. Retry with bounded backoff (60 seconds minimum; honour a longer Retry-After up to a documented maximum). Neither UI refreshes nor filter changes may multiply upstream requests. No posting, credentials, QRZ/HamQTH lookups, telemetry or new uploads are included.

### Keep imported spots separate from local decodes
Use a distinct bounded cache and web DTO for imported spots. Never insert them into the skimmer SpotBus or local PSKReporter/WSPRnet/MQTT/syslog spot reporting. Expose a receiver-local authenticated endpoint (proposed `/api/spots/external`) with enabled/state/last-success and normalized spots; follow the existing listener-password gate. Keep current local `/api/spots` and local WebSocket spot payloads compatible. Add only backwards-compatible availability information as needed so the panel works without local skimmers. Browser refresh of the receiver cache can be on a short local timer, stopped on disconnect; it does not directly poll the upstream service.

### List, labels, filters and tuning
Keep the existing list/panel. Add Source (All, Local, Spothole) and Activity filters alongside existing band/mode/text filters, with accessible names. The owner subsequently specified compact phone rows showing callsign, frequency, mode and activity only, and dropdowns hidden behind Filters by default. No phone-row comments, timestamps, source detail or references are shown; those remain available in the desktop view and accessible/title metadata. More-sheet controls wrap without horizontal overflow. Activity offers All, No activity, WWFF/POTA/SOTA, plus other observed values. A spot matches a selected activity by membership, not just its first activity. This first PR keeps the existing single-selection dropdown interaction.

Show Mode, Source and activity/reference badges or compact text. Keep SNR/DT for local decoded spots; external records have no measured local SNR and must not show a fabricated zero or "undefined". Search includes call, comment and references. Mark imported information as externally reported, not a signal decoded here. Escape all remote strings; render no upstream HTML and introduce no automatic external navigation.

Use the same combined, bounded presentation data for the existing waterfall call labels, with source-aware expiry: local lifetimes stay unchanged, external labels expire with their 30-minute feed window. List filters also filter displayed spot labels so an Activity selection gives a coherent view. Deduplicate external snapshots by upstream ID, order by UTC time deterministically and avoid multiplying the same label across repeated polls. Preserve useful simultaneous band/mode entries of one callsign.

Clicking a local spot preserves existing USB dial semantics. Clicking an external spot tunes its reported RF frequency and selects its receive mode. For upstream SSB, infer LSB below 10 MHz and USB at/above 10 MHz, with 60 m (5 MHz) using USB. Preserve explicit USB/LSB if a compatible source supplies it. Keep original SSB mode visible in the tooltip so this inference is not presented as measured fact. Users can still change mode manually. CW selects CW. No new demodulators are required.

### Alternatives needing owner choice
B. Browser-direct requests would need less server code, but each listener would contact Spothole, request volume would scale with users, browser User-Agent control is restricted, and CORS would be a runtime dependency. Not recommended for this receiver.

The SSB tuning convention above is a proposed guiding decision; the feed cannot distinguish sidebands. A manual sideband selector for every SSB click is possible but adds a new interaction.

## Change classification
Feature, UI/UX, public integration, configuration and data-model extension. This requires design confirmation before coding. Firmware trust and Kapsi hosting are not part of the feature branch.

## Docs consulted
- `doc/design.md` sections 10 and 12: hot-reload/config/auth architecture, static armv7 distribution and provenance/licensing.
- `doc/m4-skimmers.md` sections 5/6: local SpotBus, existing spot endpoints, USB dial semantics, reporter boundary and current label lifetimes.
- `ui/tools/cdp/README.md`: use a local throwaway simulated server, muted browser and targeted spot/filter/tuning checks.
- `README.md`, `.github/workflows/ci.yml`, `ui/package.json`: workspace tests/clippy/rustfmt, UI checks/tests/build and armv7 CI build.
- Spothole client documentation and OpenAPI URLs above: read API contract, rate guidance, identity and activity/mode semantics.

## Relevant files and ownership boundaries
- `crates/config/src/lib.rs`: opt-in settings, defaults/validation and live configuration classification.
- New web-side feed/cache module plus `crates/web/src/lib.rs` and `session.rs`: bounded HTTPS acquisition, authentication-compatible local endpoint and availability. Startup wiring as needed in `crates/web888d`.
- `ui/src/proto.ts`, `state.ts`, `spots.ts`, `ctl/spots.tsx`, `view/pane.tsx` and styles: separate wire payloads, normalization, filters, labels and correct tuning.
- Existing local `crates/skim` bus/reporters must not acquire imported data or change reporting semantics.
- Tests/fixtures, `doc/` and a concise PR description. No changes to the OS signing branch, remote receivers or Kapsi files.

## Assumptions and open questions
- Approved: receiver-side caching, one-minute polling, default-off configuration and the proposed SSB inference rule.
- Thirty-minute external lifetime, 60-second polling and 500 entries are proposed bounded defaults. A first snapshot rather than SSE is sufficient for this PR.
- No persistence for spot history or filter preferences is added; cache is RAM-only. Existing settings continue to work with no new outbound requests by default.
- Activity filtering is single-select to match the current list. Multiple activities on one spot remain supported.
- New source mode data is not claimed to prove the station is currently audible at this receiver.
- Scope now also includes an owner-signed test update on the existing Kapsi update source, explicitly requested by the owner after implementation began. Use the same previously approved scoped inspection/backup/staging/hash/atomic-publish/HTTPS verification process. Do not install or restart a remote receiver. Opening/submitting an upstream PR can follow review.

## Task breakdown
| Task | Purpose and areas | Dependencies | Proof | Status |
|---|---|---|---|---|
| Approve contract | Resolve acquisition and SSB policy | Research | Owner confirmation | Done |
| Provider/config/cache | Opt-in shared bounded HTTPS cache and reload | Approval | Fixture HTTP tests, timeout/backoff/disable limits | Done |
| Local API and provenance | Auth gate and separate imported DTO/cache | Provider | Integration tests; no reporter bus imports | Done |
| UI normalization/tuning | Preserve local dials; external RF + correct mode | DTO | Pure tuning/filter/expiry tests | Done |
| Panel/labels | Source/activity filters, metadata, status, phone layout | Normalization | Muted browser and screenshots | Done |
| Compatibility and PR | Existing regressions, docs, focused commit/push, PR description | All prior | CI-equivalent checks; clean diff | Done |

## Testing and verification plan
Required before calling implementation complete:
- Rust provider/config tests with a local fixture HTTP server and synthetic data: JSON arrays/units, dual activities/references, malformed/unknown modes/QRT/out-of-range/future timestamps, ID deduplication and expiry; no traffic when disabled; one fetch for many clients; timeout/body/cache bounds, 429 Retry-After/backoff, stale recovery and configuration changes. Production is HTTPS-only; any local HTTP test allowance must be internal to the test client constructor, not a public unsafe configuration.
- Web integration tests: listener auth gate, enabled feed with no local skimmers, backwards-compatible local spot routes, safe unavailable response and separation from outbound reporters.
- `pnpm test`: extend the existing spot tests with CW, both sidebands, 60 m SSB exception, multi-activity membership, source/text/reference combinations, nil SNR, deduplication, sorted merge and differing label expiry. Include a regression that FT8/WSPR still tune to the skimmer dial.
- `pnpm check` and `pnpm build`; preserve the project's WASM asset prerequisites. `cargo fmt --all --check`, targeted tests, then `cargo clippy --all-targets -- -D warnings` and `cargo test --workspace` as existing CI requires. Record unavailable toolchain checks explicitly rather than claiming passes.
- Static armv7 build with the project's existing cross-build/CI mechanism if available locally; required CI check for the added TLS client.
- Extend/add the muted CDP spots check against a local simulated receiver and synthetic external fixture (not the real Kapsi server/receiver): panel usable without skimmers, correct CW/LSB/USB/FT8 row tuning, POTA+WWFF filtering, consistent labels, error/stale/empty states, keyboard operation, 360 px and desktop widths without horizontal overflow. Inspect screenshots.
- One low-rate public API compatibility request already completed in research. Automated tests must not depend on the live Spothole service or upload any spots.

## Definition of done
The approved receiver-side/client choice is implemented. External spots appear in the existing panel and waterfall labels with correct activity/source filters, provenance, lifetimes and receive-mode tuning; local decoded spots and their reporting remain correct. Default configuration makes no new outbound requests. Bounded/failed feed behavior is honest in the UI, auth and privacy boundaries hold, and tests prove the critical rules. All tasks are Done or explicitly deferred with rationale, relevant docs are updated, and a clean reviewable feature branch plus PR description is prepared/pushed to the user's server fork. Report checks not run and remaining risks. Do not call this deployed or a completed upstream PR.

## Implementation prompt
After approval, use $project-implementation-lead with this document as the contract. Implement the provider/config/cache/API and UI slices above, preserving the existing local SpotBus/reporters and wire compatibility. Build the read client from the documented Spothole v3 contract and the repository's patterns; do not copy its application implementation. Complete the test plan with synthetic fixtures and muted local browser checks. Use the fresh app feature branch, not the owner firmware OS branch. Publication of the owner-requested signed test bundle to the already approved Kapsi directory is in scope; receiver installation/administration is not. Update this document with actual files, tests, deviations and definition-of-done evidence. Return material contract changes to Proposed for owner review.

## Implementation notes
Feature implementation complete under the approved option A, every minute. The owner then specified reuse of station identity, a Kapsi test update, and compact mobile rows with callsign/frequency/mode/activity and filters hidden by default. Source branch pushed and signed owner test release published/verified at the requested Kapsi source.

## Verification results
- Rust formatting and Clippy (`--all-targets -- -D warnings`) pass.
- Full `cargo test --workspace --locked --offline` passes on local Linux as a non-root user. The native macOS run exposed the upstream Linux-only `/proc` metrics assertion; the first Linux container run was root, incompatible with the existing read-only-file test. Neither required product changes; the correct Linux non-root run passed.
- Feed tests verify sideband inference (including 60 m), dual activity/reference parsing, malformed/expired/future/QRT/unknown/out-of-range rejection, 500-entry bounds and newest-ID deduplication, identity/header validation including private IPv6 rejection, stale/disable/source changes, response size/HTTP errors, seconds/date Retry-After and a real local TLS shared-fetch/one-minute/unrelated-reload/disable test. Listener-auth integration confirms external availability without skimmers and unchanged local-only spot API behavior.
- All 60 UI tests pass, including local FT8/WSPR dial regression, CW/LSB/USB external RF tuning, activity/source/reference combinations, nil SNR, bounded/sorted merge and source-specific label expiry.
- `pnpm check` and `pnpm build` pass with both actual decoder WASM files built by the repository script.
- Muted Chromium checks pass at 1400x900 and 360x800: source availability without local skimmers, row tuning, dual activity filters, stale/empty status, keyboard activation, no browser-side external fetch, no page/panel/list overflow; phone rows omit verbose metadata and filter dropdowns start collapsed. Screenshots inspected; More controls wrap in two columns.
- Static ARMv7 musl binary built locally using Rust 1.99.1, rust-lld and Debian ARM GCC/headers. `file` identifies statically linked ELF32 ARM EABI5. QEMU executes `--version` and `serve --print-config`, confirming new opt-in defaults. No real receiver installation was performed.
- Existing owner-signed v0.8.0 root image personalized with the exact new ARM binary and final complete UI, preserving kernel/driver/bitstream/owner trust/update helper. Inserted bytes extracted and compared, runtime trust validated and `e2fsck -fn` passed. Owner signature/full payload and public hosting verification passed: source checksums, HTTPS metadata, HTTP 206 ranges and complete public download match. The owner reported RAUC installation succeeded and confirmed reboot is still pending; do not claim active-boot/radio validation.
- Independent review found no remaining blockers after private IPv6, date-form Retry-After, VHF band mapping and station-name normalization were addressed.

## Implementation notes
The provider is a separate web cache and endpoint, with no path into the decoder SpotBus or its uploaders. The daemon starts one worker on the existing web runtime. The UI merges read-only imported data with local spots for shared filters/labels and uses a source-aware tune target. The station name inherits existing configuration, with an optional explicit override; public station URL remains explicit because no existing field can reliably supply it. No private/LAN listener URL is derived or sent.

The owner explicitly requested the test update at the existing Kapsi source under the previously approved scoped deployment procedure. This is separate from PR readiness; do not imply an upstream PR has been submitted or a receiver has been installed/restarted.

## Remaining risks
The SSB sideband convention can be wrong for unusual operation; users retain manual mode controls. External spots do not prove local audibility. Spothole is a volunteer service and can be unavailable. This is an owner test build, with real-board OTA and radio testing still pending. Before reverting to an older daemon after saving the new config schema, restore the prior config/remove the new `[spots.spothole]` table, since older releases reject unknown fields. Owner test release v0.8.0-owner2-spothole1 is signed and published. Owner reported successful inactive-slot installation; reboot/radio testing is pending.

## Final delivery
Feature source commit 9d6da36 on j0uni/web888-ng branch jouni/spothole-spots; PR description prepared, no upstream PR submitted. Owner test bundle v0.8.0-owner2-spothole1 is public at https://jouni.kapsi.fi/web888fw/latest.raucb (91,953,268 bytes; SHA-256 bd83b1c6f3c42d57ea00514b7745e74248ad5ae3d55621048683f7b422ea3a5f). Previous owner1 remains available and hosting rollback copies were preserved. Agent did not install or restart a receiver. Owner has been advised to reboot to activate the already-installed slot, then enable Spothole and set a public station_url.
