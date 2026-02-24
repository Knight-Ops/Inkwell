# Inkwell

High-performance trading card scanner and price tracker built with Rust.

## Key Features

- **Real-time Identification**: Uses the AKAZE algorithm for fast and accurate card matching via camera feed.
- **Price Tracking**: Integrates with the Lorcast API to provide live market pricing (Normal and Foil).
- **Global Statistics**: Persistently tracks total scans and session value.
- **CSV Export**: Easily export your scanned collection to CSV format.
- **Docker Ready**: One-command deployment via Docker Compose with Cloudflare Tunnel support.

## Tech Stack

- **Frontend**: Rust (Leptos WASM), Tailwind CSS
- **Backend**: Rust (Axum, SQLx SQLite)
- **Engine**: OpenCV (AKAZE feature matching)
- **Deployment**: Docker, cloudflared

## Quick Start

1. Ensure you have Docker and Docker Compose installed.
2. Update the `.env` file in the `deploy/` directory with your `TUNNEL_TOKEN`.
3. Run the application:
   ```bash
   cd deploy
   docker compose up --build
   ```
4. Access the scanner at the hostname configured in your tunnel or locally at `http://localhost:4000`.

## Project Structure

- `inkwell-client`: Leptos-based WASM web application.
- `inkwell-server`: Axum API server for image processing and database management.
- `inkwell-core`: Shared logic, types, and feature extraction utilities.
- `migrations`: SQLx database migrations for schema and statistics.
