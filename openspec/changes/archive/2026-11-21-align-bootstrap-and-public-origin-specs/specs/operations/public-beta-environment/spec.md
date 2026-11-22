## MODIFIED Requirements

### Requirement: Public traffic uses trusted HTTPS
The platform's own public beta origin SHALL be exactly
`https://cloud-test.makodb.com`, and it SHALL be the only origin that serves
the console, the management API and the operator API. For that origin the
deployment MUST obtain a publicly trusted certificate only after DNS and
reachability preflight succeeds, MUST redirect plain HTTP to HTTPS except for
required certificate validation traffic, and MUST renew the certificate
automatically before expiry.

Developers' verified custom domains are additional hostnames. They SHALL serve
only what `operations/custom-domains` allows, and a certificate for one SHALL
be requested on demand and only after the control plane confirms that the
hostname is verified.

Certificate issuance or renewal failure MUST be observable and MUST NOT expose
an unencrypted Mako API fallback on any hostname.

#### Scenario: Initial certificate issuance succeeds
- **WHEN** the VM owns `130.245.173.11`, DNS resolves correctly, and the certificate authority can validate the hostname
- **THEN** the public origin serves a valid certificate for `cloud-test.makodb.com` and HTTP requests redirect to HTTPS

#### Scenario: Certificate cannot be renewed
- **WHEN** automated renewal fails or the remaining certificate lifetime crosses the alert threshold
- **THEN** operators receive an alert and the deployment never falls back to serving application traffic over plain HTTP
