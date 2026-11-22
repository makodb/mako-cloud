# Database Webhooks Specification

## Purpose

Tell external systems when data changes: signed HTTP deliveries on collection events, durable enough to trust and observable enough to debug.
## Requirements
### Requirement: Webhook endpoints on collection changes
An authorized member SHALL be able to register an HTTPS endpoint for an environment, subscribed to insert, update, and delete events of chosen collections. The platform SHALL generate the endpoint's signing secret, store it sealed, and show it exactly once, in the response that creates the endpoint. The secret MUST NOT be listed, logged or returned afterwards. It SHALL be replaceable only by rotation, which shows the new secret once, advances the endpoint's secret version, and signs every delivery attempted from then on with the new secret only.

Deliveries SHALL carry the event, the collection, the document identifier and revision, and a signature over the body, and MUST NOT carry document fields.

#### Scenario: A document change is delivered
- **WHEN** a subscribed collection accepts a document write
- **THEN** the endpoint receives one signed delivery describing the change within the delivery objective, and the delivery is logged with its response status

#### Scenario: The signing secret is rotated
- **WHEN** an authorized member rotates an endpoint's signing secret
- **THEN** a new secret is shown once, the endpoint's secret version advances, and every delivery attempted afterwards verifies only with the new secret and names its version

### Requirement: Durable delivery with retries
Deliveries SHALL be durable across restarts, retried with backoff on failure for a bounded period, and delivered at least once in order per document. An endpoint that keeps failing SHALL be paused and the pause surfaced, never silently dropped.

#### Scenario: An endpoint is temporarily down
- **WHEN** the endpoint refuses connections for a few minutes and then recovers
- **THEN** every change from that window is delivered after recovery and the delivery log shows the retries

### Requirement: Delivery log
The console SHALL show recent deliveries per endpoint with time, event, status, attempts, and response code, and allow redelivery of a failed one.

#### Scenario: A developer redelivers a failed event
- **WHEN** an authorized member redelivers a failed delivery
- **THEN** a new signed delivery is attempted and logged as a redelivery

