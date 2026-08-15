---
type: Rust Function
title: process
resource: src/pipeline.rs#L8
relationships:
  calls:
    - target: functions/pipeline/validate
      resolved_by: tree-sitter
      confidence: exact
    - target: functions/pipeline/dispatch
      resolved_by: rust-analyzer
      confidence: semantic
      resolver_version: 1.88.0
---

# Signature

`fn process(input: &str) -> Result<()>`

# Calls

- [validate](validate.md)
- [dispatch](dispatch.md)
