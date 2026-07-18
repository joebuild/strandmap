# Session tokens

Session tokens contain a subject and an expiration interval. Issuers serialize
both claims, and verifiers reject tokens missing either claim.

The schema is maintained in `schema/session-token.json`.
