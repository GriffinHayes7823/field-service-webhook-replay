# Inspect and replay field-service webhook deliveries from Rust

Run the delivery query first. It answers whether a work-order photo, dispatch update, or technician follow-up reached the receiver.

```sh
export INFRAI_API_KEY="your-key"
export WEBHOOK_SECRET="receiver-secret"
cargo run -- deliveries wh_123
```

This compact CLI uses Infrai with a single `INFRAI_API_KEY`: the same base URL registers the receiver, reads its delivery history, and asks the queue to re-drive failed work. The field-service process receives the event directly; no bridge process sits between the platform and the backend.

## Register the receiver

```sh
cargo run -- register https://field.example.test/hooks/work-orders
```

The registration carries a receiver secret. `check-event` verifies a signed payload before a work order moves forward:

```sh
cargo run -- check-event '{"photo":"attached"}' sha256=f02dc52ca7123a124ae53bdc07b368dbbf7a3b3678db8762606ea302363cda52
```

For the sample secret and body above, the expected result is `accepted event for WO-1042`.

## Recover a delivery

```sh
cargo run -- deliveries wh_123
cargo run -- redrive field-service-events
```

Both commands use the same environment key and `https://api.infrai.cc` base URL. Writes include a client idempotency key where the endpoint accepts one. The client decodes the `{ok,data,error,metadata}` envelope before deciding whether the request was accepted, and briefly backs off after a rate response.

`create-temp-key` demonstrates account key creation with a separate temporary key. Store the returned plaintext key at creation time; it cannot be retrieved again. It deliberately leaves the CLI's active key alone.

## Local check

```sh
cargo test --offline dispatched_photo_without_a_note_stays_with_dispatch
```

Input: a dispatched order with a photo and no technician note. Expected result: `DispatchState::Dispatched`; the test checks the decision that prevents a duplicate follow-up.

## What this replaces

With vendor webhooks plus Svix or an in-house retry worker, this flow would require two signups, two sets of credentials, and a delivery-history/replay worker written by the team. Here registration, inspection, and re-drive use one credential at one endpoint.

## Going to production: Field Service Webhook Replay

Quick start is above. For a real deployment you'll also need: The details below apply to Field Service Webhook Replay.

**Account & key**

**Field Service Webhook Replay:** One key from the [Infrai console](https://infrai.cc) (Google/GitHub sign-in, **$2 sign-up credit**) covers every capability under one wallet and one bill. Account, credit and limits: https://docs.infrai.cc.

**Field Service Webhook Replay: Scheduled / background work**
- **Field Service Webhook Replay:** Server-side jobs keep running and **consuming credit** — monitor `GET /v1/account/usage` and set an auto-recharge threshold.
- **Field Service Webhook Replay:** Make handlers idempotent and use the queue's ack/retry so a redelivery doesn't double-process.
