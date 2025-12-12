# ADK-UI: Dynamic UI Generation

## Overview

`adk-ui` enables agents to dynamically generate rich user interfaces via tool calls. Agents can render forms, cards, alerts, tables, charts, and more - all through a type-safe Rust API that serializes to JSON for frontend consumption.

## Current State (v0.1.5)

### What Works

- **28 Component Types**: Full schema with optional IDs for streaming updates
- **10 Render Tools**: `render_form`, `render_card`, `render_alert`, `render_confirm`, `render_table`, `render_chart`, `render_layout`, `render_progress`, `render_modal`, `render_toast`
- **Recharts Integration**: Real charts (bar, line, area, pie) via Recharts library
- **Bidirectional Data Flow**: Forms submit data back to agent via `UiEvent`
- **Streaming Protocol**: `UiUpdate` type for incremental component updates by ID
- **TypeScript Client**: React renderer in `examples/ui_react_client`
- **Unit Tests**: 13 tests covering schema serialization, variants, updates, and toolset

### Architecture

```
Agent ──[render_* tool]──> UiResponse ──[SSE]──> React Client
               ↑                                      │
               └────────── UiEvent ◄──────────────────┘

Streaming Updates:
Agent ──[UiUpdate]──> Client ──[patch by ID]──> DOM
```

### Components

**Atoms**: Text, Button, Icon, Image, Badge
**Inputs**: TextInput, NumberInput, Select, MultiSelect, Switch, DateInput, Slider, Textarea
**Layouts**: Stack, Grid, Card, Container, Divider, Tabs
**Data**: Table, List, KeyValue, CodeBlock
**Visualization**: Chart (bar, line, area, pie via Recharts)
**Feedback**: Alert, Progress, Toast, Modal, Spinner, Skeleton

### Usage

```rust
use adk_rust::prelude::*;
use adk_rust::ui::UiToolset;

let agent = LlmAgentBuilder::new("ui_agent")
    .model(model)
    .tools(UiToolset::all_tools())
    .build()?;
```

## Known Limitations

1. **React-only client** - No Vue/Svelte/vanilla JS renderers
2. **Manual integration** - React client must be copied from examples
3. **No accessibility** - Missing ARIA attributes
4. **No server-side rendering** - Client-side only
5. **No component validation** - Schema validation is client-side only

## Future Work

### Phase 2: Enhanced Forms
- [x] Form validation rules (min/max length) - `TextInput.min_length`, `TextInput.max_length`
- [ ] Autocomplete/combobox input
- [ ] Date range picker
- [ ] Color picker
- [ ] File upload with preview

### Phase 3: Data Display
- [x] Table pagination - `Table.page_size`
- [x] Sortable table columns - `Table.sortable`, `TableColumn.sortable`
- [ ] Timeline component
- [ ] Avatar component

### Phase 4: Navigation & Layout
- [ ] Accordion (collapsible sections)
- [ ] Stepper (multi-step wizard)
- [ ] Carousel (image/content slider)
- [ ] Tooltip

### Phase 5: Advanced
- [ ] Rating component (star ratings)
- [ ] Drag & drop reorderable lists
- [ ] Rich text editor

### Infrastructure
- [ ] Publish React client as npm package (`@adk/ui-react`)
- [ ] ARIA accessibility attributes
- [ ] Server-side rendering support
- [ ] Multi-framework clients (Vue, Svelte)
- [ ] Component validation in Rust
- [ ] Theming API expansion

## Files

- `adk-ui/src/schema.rs` - Component types and UiUpdate
- `adk-ui/src/toolset.rs` - UiToolset configuration
- `adk-ui/src/tools/` - Individual render tools

## Examples

### `examples/ui_agent/`
Console-based demo agent with UI tools. Runs in terminal via `adk_cli::console`.

```bash
GOOGLE_API_KEY=... cargo run --example ui_agent
```

### `examples/ui_server/`
HTTP server exposing UI agent via SSE. Uses `adk_cli::serve` for REST API.

```bash
GOOGLE_API_KEY=... cargo run --example ui_server
# Server runs on http://localhost:8080
```

### `examples/ui_react_client/`
React frontend that connects to ui_server and renders UI components.

```bash
cd examples/ui_react_client
npm install && npm run dev
# Client runs on http://localhost:5173
```

**Full stack demo**: Run ui_server in one terminal, ui_react_client in another, then interact via browser.

### v0.1.6 (2025-12-12)
- **Schema Enhancements**: 
  - Added `icon` field to `Button` component
  - Added `id` field to `Divider` component (consistency)
  - Added `min_length`, `max_length` to `TextInput`
  - Added `default_value` to `NumberInput`
  - Added `sortable`, `striped`, `page_size` to `Table`
  - Added `sortable` to `TableColumn`
  - Added `x_label`, `y_label`, `show_legend`, `colors` to `Chart`
- **render_layout enhancements**:
  - Added `key_value` section type
  - Added `list` section type
  - Added `code_block` section type
- **Error handling**: Replaced `unwrap()` with proper error handling in all 10 tools
- **Documentation**: Added rustdoc examples to 6 tools with JSON parameter examples
- TypeScript types updated for all new schema fields

### v0.1.5 (2025-12-12)
- **Phase 1 Complete**: Critical UX features implemented
- Added 5 new components: Toast, Modal, Spinner, Skeleton, Textarea
- Added 3 new enums: ModalSize, SpinnerSize, SkeletonVariant
- Added 2 new tools: `render_modal`, `render_toast`
- Integrated Recharts for real charts (bar, line, area, pie)
- Updated `render_form` to support textarea field type
- Now 28 component types and 10 render tools
- Added component IDs (`id: Option<String>`) to all components
- Added `UiUpdate` type for streaming incremental updates
- Added `UiOperation` enum: Replace, Patch, Append, Remove
- Fixed `BadgeVariant` to include: default, info, success, warning, error, secondary, outline
- Fixed `KeyValue` to use `pairs` field (renamed from `items`)
- Renamed `KeyValueItem` to `KeyValuePair` for consistency
- Added 13 unit tests (8 schema, 5 toolset)
- Integrated into umbrella crate with `ui` feature flag
- Added to prelude: `UiToolset`
- TypeScript types updated to match Rust schema
- React client uses Recharts instead of CSS-only charts
- Added markdown rendering for text components

### v0.1.0 (2025-12-11)
- Initial implementation with 8 tools and 23 component types
