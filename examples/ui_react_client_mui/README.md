# MUI Client (Enterprise/Themed)

This is an alternative frontend client for ADK UI examples, built with **React** and **Material UI (MUI)**. It provides a polished, enterprise-grade interface for interacting with ADK agents.

## Features

*   **Material Design**: Uses MUI components for a consistent and professional look.
*   **Themed**: Supports light/dark mode (configured in `App.tsx`).
*   **Multi-Agent Support**: Connects to various backend examples (UI Demo, Support, Appointments, etc.).

## Setup & Running

1.  Install dependencies:
    ```bash
    npm install
    ```

2.  Start the development server:
    ```bash
    npm run dev
    ```

    The application will be available at [http://localhost:3001](http://localhost:3001).

## Configuration

The client connects to backend services defined in `src/App.tsx`. You can configure the API URL via `.env` file (see `.env.example`).

![Screenshot](path/to/screenshot.png)
