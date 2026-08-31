---
id: q-11ba
title: Stripe webhooks replay in staging
paths: [src/billing/**]
severity: landmine        # landmine | gotcha | debt
status: active
source: t-8812
fixed_by: null
---
Retries are not idempotent before the ledger write.
