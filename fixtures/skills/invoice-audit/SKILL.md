---
name: invoice-audit
description: Validate invoice totals and write a deterministic audit summary.
license: Apache-2.0
compatibility: python3 >= 3.9
allowed-tools: python3
metadata:
  owner: finance
  custom-field: preserve-me
---

# Invoice Audit

Read the JSON input, sum the invoice line items, and write `summary.json` into the artifacts directory.

Use this skill when a deterministic, offline invoice total check is required.
