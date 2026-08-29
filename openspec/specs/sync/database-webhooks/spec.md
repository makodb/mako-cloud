# Database Webhooks Specification

## Purpose

Tell external systems when data changes: signed HTTP deliveries on collection events, durable enough to trust and observable enough to debug.

## Requirements

### Requirement: Webhook endpoints on collection changes
An authorized member SHALL be able to register an HTTPS endpoint for an environment, subscribed to insert, update, and delete events of chosen collections, with a signing secret stored as a reference and shown once. Deliveries SHALL carry the event, the collection, the document identifier and revision, and a signature over the body, and MUST NOT carry document fields the endpoint's policy scope does not allow.

#### Scenario: A document change is delivered
- **WHEN** a subscribed collection accepts a document write
- **THEN** the endpoint receives one signed delivery describing the change within the delivery objective, and the delivery is logged with its response status

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
