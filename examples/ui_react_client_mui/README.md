# Enterprise/Themed React Client (MUI)

This example provides a polished, enterprise-ready React frontend for ADK agents, built with **Material UI (MUI)**. It serves as a more feature-rich and visually consistent alternative to the standard `ui_react_client`.

## Features

- **Material UI**: Uses standard MUI components for a consistent, professional look.
- **Theming**: Easily customizable theme.
- **Components**: Includes enhanced chat interface, settings panels, and visualization components.
- **Vite**: Fast build and dev server.

## Screenshot

![MUI Client Interface](screenshot_placeholder.png)

## Getting Started

### Prerequisites

- Node.js (v18+)
- npm or pnpm

### Installation

```bash
cd examples/ui_react_client_mui
npm install
```

### Running the Client

Start the development server:

```bash
npm run dev
```

The application will be available at **http://localhost:5174** (to avoid conflict with the standard client on port 3000/5173).

## Configuration

Copy `.env.example` to `.env` to configure the backend connection:

```bash
cp .env.example .env
```

Ensure your ADK agent backend is running and accessible (default: `http://localhost:8080`).
