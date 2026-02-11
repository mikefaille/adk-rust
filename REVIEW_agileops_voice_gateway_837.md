# Review of Docker Build Failure (agileops/voice_gateway#837)

## Issue Analysis
The build fails during the `cargo chef cook` step with the error:
```
Caused by:
  failed to read `/app/zenith/adk-rust/adk-agent/Cargo.toml`
  No such file or directory (os error 2)
```

## Root Cause
The `cargo chef cook` command attempts to build dependencies, including local path dependencies. The error indicates that the `adk-agent` crate (located at `zenith/adk-rust/adk-agent` relative to the build context) is missing in the Docker container's `builder` stage.

While `cargo chef prepare` (planner stage) generates a recipe, `cargo chef cook` (builder stage) requires the actual source code of local dependencies to compile them.

## Recommended Fix
You need to explicitly `COPY` the `adk-rust` directory (or the specific `adk-agent` crate) into the container before running `cargo chef cook`.

Add the following instruction to your Dockerfile (adjusting the source path as necessary based on your repository structure):

```dockerfile
# In the builder stage, before "RUN ... cargo chef cook ..."
COPY adk-rust /app/zenith/adk-rust
# OR if adk-rust is in the root of the build context:
COPY adk-rust zenith/adk-rust
```

Ensure that the destination path matches where `Cargo.toml` expects the dependency to be (which appears to be `../adk-rust/adk-agent` relative to `zenith/data_plane`, i.e., `zenith/adk-rust/adk-agent`).
