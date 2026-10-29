# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Entries are generated from
[Conventional Commits](https://www.conventionalcommits.org/) by release-please and reviewed by a human.

## [0.3.0](https://github.com/Panacota96/rv-rsmf/compare/v0.2.0...v0.3.0) (2026-10-29)


### Features

* **cli:** describe --json prints the tool's self-description for RV Hub ([02b01f1](https://github.com/Panacota96/rv-rsmf/commit/02b01f1c8964b543f5e648be2b0e85d4f8f5a0d4))
* **gui:** --case-dir DIR presets the case folder when RV Hub launches the app ([c6d2c6c](https://github.com/Panacota96/rv-rsmf/commit/c6d2c6c09d369a25cd80fd655ef2b51292ce0ad1))

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

### Added
- Project scaffolded with rakvor-tool-forge (rust profile) on 2026-10-27.
