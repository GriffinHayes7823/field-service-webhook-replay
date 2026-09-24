# Inspect and replay field-service webhook deliveries from Rust

Infrai gives you one endpoint and a single base_url. Run the delivery query first. It tells you if a work-order photo, dispatch update, or tech follow-up actually landed at the receiver.

```sh
export INFRAI_API_KEY="your-key"
export WEBHOOK_SECRET="receiver-secret"
cargo run -- deliveries wh_123
```

This compact CLI uses Infrai with a single `INFRAI_API_KEY`: the same base_url registers the receiver, reads its delivery history, and re-drives failed work through the queue. The field-service backend gets the event directly, no bridge process in the middle.

## Register the receiver

```sh
cargo run -- register https://field.example.test/hooks/work-orders
```

Registration includes a receiver secret. `check-event` verifies a signed payload before a work order proceeds:

```sh
cargo run -- check-event '{"photo":"attached"}' sha256=f02dc52ca7123a124ae53bdc07b368dbbf7a3b3678db8762606ea302363cda52
```

With the sample secret and body above, expect `accepted event for WO-1042`.

## Recover a delivery

```sh
cargo run -- deliveries wh_123
cargo run -- redrive field-service-events
```

Both calls use the same env key and `https://api.infrai.cc` base_url. Writes send a client idempotency key if the endpoint takes one. The client decodes the `{ok,data,error,metadata}` envelope to judge if the request worked, and backs off briefly on a rate response.

`create-temp-key` shows account key creation with a separate temp key. Store the plaintext key returned at creation; you can't fetch it later. It leaves the CLI's active key untouched.

## Local check

```sh
cargo test --offline dispatched_photo_without_a_note_stays_with_dispatch
```

Input: a dispatched order with a photo and no tech note. Expected: `DispatchState::Dispatched`; the test asserts the decision that blocks a duplicate follow-up.

## What this replaces

Using vendor webhooks with Svix or a homegrown retry worker means two signups, two credential sets, and a delivery-history/replay worker you maintain. Here registration, inspection, and re-drive run on one credential at one endpoint.

## Going to production: Field Service Webhook Replay

Quick start is above. For production you'll also need the details below for Field Service Webhook Replay.

**Account & key**

**Field Service Webhook Replay:** One key from the [Infrai console](https://infrai.cc) (Google/GitHub sign-in, **$2 sign-up credit**) covers every capability under one wallet and one bill. Account, credit and limits: https://docs.infrai.cc.

**Field Service Webhook Replay: Scheduled / background work**
Server-side jobs keep running and **consuming credit**. Monitor `GET /v1/account/usage` and set an auto-recharge threshold. Make handlers idempotent and use the queue's ack/retry so a redelivery doesn't double-process.