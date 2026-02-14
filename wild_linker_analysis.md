# Wild Linker Incremental Analysis

## Version
The installed version of the `wild` linker is 0.8.0.
Binary path: `/home/jules/.cargo/bin/wild` (verified as the primary binary in `PATH` within the devenv environment).

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

## Verification
To ensure the binary was actually linked with Wild and not a system default, check the `.comment` section:

```bash
readelf -p .comment <your_binary>
```

You should see a string similar to:
`Linker: Wild version 0.8.0`

## Limitations
Combining `--update-in-place` with certain features like Section Garbage Collection (`--gc-sections`) may be disabled or limited because it adds significant complexity to the incremental mapping.

## Conclusion
Based on the analysis of `wild` v0.8.0, `WILD_INCREMENTAL` is not used. The correct flag for incremental linking is `--update-in-place`.
