# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Entries are generated from
[Conventional Commits](https://www.conventionalcommits.org/) by release-please and reviewed by a human.

## [0.3.2](https://github.com/Panacota96/rv-rsmf/compare/v0.3.1...v0.3.2) (2026-11-14)


### Bug Fixes

* **cli:** verify-log and verify-case print the outputs line and document the re-hash ([2e5c115](https://github.com/Panacota96/rv-rsmf/commit/2e5c1157f61afdecf7e3b0f7af6ebe0691cc3dd2))
* **core:** late audit entries and redirected run folders cannot hide an edited output ([2869a0e](https://github.com/Panacota96/rv-rsmf/commit/2869a0e0a816887da420492df1b723fec7e6e665))
* **core:** re-hash the outputs a run's audit log records in verify-log and verify-case ([f6d4d5a](https://github.com/Panacota96/rv-rsmf/commit/f6d4d5ab4bf9c5768918d47ced78c7a24cd86a4a))
* verify-log and verify-case re-hash the outputs the audit log records ([7c07eb3](https://github.com/Panacota96/rv-rsmf/commit/7c07eb3615d0ab29f9d14491e822c95e93f2d011))

## [0.3.1](https://github.com/Panacota96/rv-rsmf/compare/v0.3.0...v0.3.1) (2026-11-10)


### Bug Fixes

* **security:** patch dependency advisories, blind RSA signing, check advisories in CI ([2277631](https://github.com/Panacota96/rv-rsmf/commit/22776316a061ab41cb30a374012d3f708de81f54))
* **security:** patch dependency advisories, blind RSA signing, check advisories in CI ([ed62ad4](https://github.com/Panacota96/rv-rsmf/commit/ed62ad48bb4ce67a6e42359f04a287d697fc1077))

## [0.3.0](https://github.com/Panacota96/rv-rsmf/compare/v0.2.0...v0.3.0) (2026-11-10)


### Features

* **cli:** describe --json prints the tool's self-description for RV Hub ([02b01f1](https://github.com/Panacota96/rv-rsmf/commit/02b01f1c8964b543f5e648be2b0e85d4f8f5a0d4))
* **gui:** --case-dir DIR presets the case folder when RV Hub launches the app ([c6d2c6c](https://github.com/Panacota96/rv-rsmf/commit/c6d2c6c09d369a25cd80fd655ef2b51292ce0ad1))


### Bug Fixes

* **trust:** drive timestamp checks from the audit log; RFC 3161/5280 TSA path rules ([aa104e4](https://github.com/Panacota96/rv-rsmf/commit/aa104e49c8c939d119669eca1f1c278176f9e12c))
* **trust:** verify the TSA CMS signature and, with --tsa-ca, its chain ([f188b87](https://github.com/Panacota96/rv-rsmf/commit/f188b87ec5c7beb53bfc9b32d6e6e908d4357b32))
* **trust:** verify the TSA CMS signature and, with --tsa-ca, its chain ([d8fbd9f](https://github.com/Panacota96/rv-rsmf/commit/d8fbd9f9b8d0446eaaeedf2ec51239f2ca1ed565))

## [0.2.0](https://github.com/Panacota96/rv-rsmf/compare/v0.1.0...v0.2.0) (2026-10-27)


### Features

* conversion stage, convert/preview/validate commands and one-step rv-hashinv intake ([3e83b9a](https://github.com/Panacota96/rv-rsmf/commit/3e83b9aa4b22f0e97747a1163a0bbe614ec8c065))
* **core:** freeze the chat model, RSMF 2.0 writer/reader/validator and finding catalogue ([afd9708](https://github.com/Panacota96/rv-rsmf/commit/afd970887708c4240bca99a340950975c6a86e0a))
* **core:** parse Teams .msg exports and mapped CSV/TSV chat exports ([a401150](https://github.com/Panacota96/rv-rsmf/commit/a40115091716634c3829338891ccc21a233ccd36))
* **gui:** chat-export settings, preview panel, RSMF results and RV HashInv intake ([3fea6b0](https://github.com/Panacota96/rv-rsmf/commit/3fea6b058d5579b6dbe9e0b94925cdb7dea55976))
* **gui:** preview files given at launch; screenshot of a Telegram preview ([3c5a6a7](https://github.com/Panacota96/rv-rsmf/commit/3c5a6a73de891c6f645d95eada3f136102558f41))
* **parsers:** WhatsApp (.txt/.zip, Android/iOS) and Telegram Desktop JSON parsers ([18d8948](https://github.com/Panacota96/rv-rsmf/commit/18d8948aa684aa67c02c57369efd332ef70d45cc))
* **reports:** RSMF conversion section in technical reports, summaries and statistics ([95c02bd](https://github.com/Panacota96/rv-rsmf/commit/95c02bd12efb70a6575a29393ce3a207f5d4ab9d))


### Bug Fixes

* replies across days, unreadable attachments, DST gaps of any length, CSV CRLF lines; signed release verify ([92ae9a8](https://github.com/Panacota96/rv-rsmf/commit/92ae9a89b9aa63e9bbd872dceea57e39fc374da3))

## [Unreleased]

### Fixed
- Timestamp verification (`verify-log`, `verify-case`) is driven by the audit log's `timestamp_obtained`
  entries: deleting a token's `timestamp-<SEQ>.json` no longer skips its signature and chain checks (with `--tsa-ca`
  an untrusted TSA passed that way). A recorded token whose `.tsr` or `.json` is missing, or whose metadata disagrees
  with the entry, is a problem (exit 3); a token file that nothing records is checked too, or is a problem when it
  cannot be.
- The TSA certificate's extended key usage must be critical and contain only `id-kp-timeStamping` (RFC 3161 §2.3),
  as in rv-certcheck.
- `--tsa-ca` path validation enforces `pathLenConstraint` (anchor included, self-issued certificates not counted),
  `keyCertSign` for issuers with keyUsage, directoryName name constraints, and rejects unsupported critical
  extensions on any certificate of the path (policy processing and other name forms are not implemented: such
  chains fail closed). Path building tries every candidate issuer and anchor (cross-signed intermediates, renewed
  anchors, self-issued key-rollover intermediates), at most 8 certificates deep, without loops. Tested against 34
  fixture chains cross-checked with `openssl verify -purpose timestampsign` (`tests/fixtures/tsa-chains`).
- Docs: the `openssl ts -verify` cross-check is labelled a signature/chain check; revocation of the TSA certificate
  is never checked by the tool and must be established from CRL/OCSP evidence covering genTime.
- `verify-log` and `verify-case` now verify the TSA's CMS signature on every RFC 3161 token (signer certificate
  in the token, ESS signing-certificate binding, message digest over TSTInfo, RSA PKCS#1 v1.5 signature,
  `timeStamping` key usage); before, a token with an altered signature was reported intact. `verify-log --tsa-ca
  <PEM>` also validates the TSA certificate chain at genTime; without it the output says "chain not checked".
  Unreadable bundle: exit 1; signature or chain failure: exit 3. Revocation is not checked; RSA signers only.

### Added
- Project scaffolded with rakvor-tool-forge (rust profile) on 2026-10-27.
