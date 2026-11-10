# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Entries are generated from
[Conventional Commits](https://www.conventionalcommits.org/) by release-please and reviewed by a human.

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
