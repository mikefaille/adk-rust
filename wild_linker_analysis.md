# Wild Linker Incremental Analysis

## Version
The installed version of the `wild` linker is 0.8.0.

## Findings
- **WILD_INCREMENTAL**: The environment variable `WILD_INCREMENTAL` was **not found** in the `wild` binary strings or help output. It does not appear to be a valid configuration option for this version of the tool.
- **Codebase Usage**: The codebase does not reference `WILD_INCREMENTAL` anywhere.
- **Incremental Linking**: The linker supports `--update-in-place`, which likely implements the desired incremental linking behavior.
- **Other Environment Variables**: The following `WILD_` variables were found in the binary strings:
    - `WILD_SAVE_DIR`
    - `WILD_SAVE_BASE`
    - `WILD_FILES_PER_GROUP`
    - `WILD_VALIDATE_OUTPUT`
    - `WILD_WRITE_LAYOUT`
    - `WILD_VERIFY_ALLOCATIONS`
    - `WILD_PRINT_ALLOCATIONS`
    - `WILD_PERFETTO_OUT`
    - `WILD_REFERENCE_LINKER`

## Conclusion
Based on the analysis of `wild` v0.8.0, `WILD_INCREMENTAL` is not used. The user might be referring to a different linker (e.g., Mold uses `MOLD_INCREMENTAL`) or a newer version of Wild.
