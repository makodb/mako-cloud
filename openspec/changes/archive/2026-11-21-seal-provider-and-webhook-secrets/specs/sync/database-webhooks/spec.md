## MODIFIED Requirements

### Requirement: Webhook endpoints on collection changes
An authorized member SHALL be able to register an HTTPS endpoint for an environment, subscribed to insert, update, and delete events of chosen collections. The platform SHALL generate the endpoint's signing secret, store it sealed, and show it exactly once, in the response that creates the endpoint. The secret MUST NOT be listed, logged or returned afterwards. It SHALL be replaceable only by rotation, which shows the new secret once, advances the endpoint's secret version, and signs every delivery attempted from then on with the new secret only.

Deliveries SHALL carry the event, the collection, the document identifier and revision, and a signature over the body, and MUST NOT carry document fields.

#### Scenario: A document change is delivered
- **WHEN** a subscribed collection accepts a document write
- **THEN** the endpoint receives one signed delivery describing the change within the delivery objective, and the delivery is logged with its response status

#### Scenario: The signing secret is rotated
- **WHEN** an authorized member rotates an endpoint's signing secret
- **THEN** a new secret is shown once, the endpoint's secret version advances, and every delivery attempted afterwards verifies only with the new secret and names its version
