---
type: Rust Function
title: verify_token
resource: src/auth.rs#L4
relationships:
  calls:
    - target: functions/auth/decode_jwt
      resolved_by: tree-sitter
      confidence: exact
---

# Signature

`fn verify_token(token: &str) -> bool`

# Calls

- [decode_jwt](decode_jwt.md)
