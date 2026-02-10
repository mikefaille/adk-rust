# Enterprise/Themed React Client (MUI)

This example provides a polished, enterprise-ready React frontend for ADK agents, built with **Material UI (MUI)**. It serves as a more feature-rich and visually consistent alternative to the standard `ui_react_client`.

## Features

- **Material UI**: Uses standard MUI components for a consistent, professional look.
- **Theming**: Easily customizable theme.
- **Components**: Includes enhanced chat interface, settings panels, and visualization components.
- **Vite**: Fast build and dev server.

## Screenshot

![MUI Client Interface](screenshot_placeholder.png)

## Quick Start

```bash
# Start the UI server (in adk-rust root)
GOOGLE_API_KEY=... cargo run --example ui_server

# In another terminal, start this client
cd examples/ui_react_client_mui
npm install
npm run dev
```

The application will be available at **http://localhost:5174** (to avoid conflict with the standard client on port 3000/5173).

## Configuration

Copy `.env.example` to `.env` to configure the backend connection:

```bash
cp .env.example .env
```

Ensure your ADK agent backend is running and accessible (default: `http://localhost:8080`).

## What This Does

This client connects to the ADK UI server via SSE and renders UI components that agents generate through `render_*` tool calls:

- **Forms** - User input with text fields, selects, switches, etc.
- **Cards** - Information display with action buttons
- **Alerts** - Success, warning, error, and info notifications
- **Tables** - Tabular data display
- **Charts** - Bar, line, area, and pie charts
- **Progress** - Step-by-step task progress
- **Layouts** - Dashboard-style multi-section views

## Architecture

```
┌─────────────────┐     SSE      ┌──────────────┐
│  React Client   │◄────────────│  ui_server   │
│   (Vite)        │             │  (Rust)      │
│                 │────POST────►│              │
└─────────────────┘  /api/run   └──────────────┘
         │                              │
         ▼                              ▼
   Renderer.tsx                  LlmAgent + UiToolset
```

## Key Files

- `src/adk-ui-renderer/types.ts` - TypeScript types matching Rust schema
- `src/adk-ui-renderer/Renderer.tsx` - Component renderer (23 components)
- `src/App.tsx` - Main app with SSE connection

## Customization

The renderer uses Material UI (MUI) and Tailwind CSS. Modify `Renderer.tsx` or the theme configuration to customize styling or add new component types.

## Production Build

```bash
npm run build
# Output in dist/
```
