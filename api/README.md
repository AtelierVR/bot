# Nox API

Rust client library for interacting with the Nox platform.

## Features

- 🔍 **Automatic Gateway Discovery** - Automatically discover node gateways via DNS TXT records and .well-known endpoints
- 🌐 **HTTP/HTTPS Support** - Full support for both HTTP and HTTPS connections
- 🔄 **Fallback Strategies** - Multiple discovery strategies with automatic fallback
- ⚡ **Async/Await** - Built on top of `tokio` and `reqwest` for async operations

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
noxapi = { path = "../api" }
```

## Usage

### Basic Usage

```rust
use noxapi::Nox;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create a client with a known gateway URL
    let client = Nox::new("https://gateway.example.com");
    
    // Get user information
    let response = client.get_user_by_username("john_doe").await;
    
    if let Some(user) = response.data {
        println!("User: {} (ID: {})", user.display_name, user.id);
    }
    
    Ok(())
}
```

### Automatic Gateway Discovery

The library can automatically discover the gateway URL from a domain name:

```rust
use noxapi::Nox;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Automatically discover the gateway for example.com
    let client = Nox::with_discovery("example.com").await?;
    
    // Use the client normally
    let response = client.get_user_by_username("john_doe").await;
    
    Ok(())
}
```

### Manual Gateway Discovery

You can also use the discovery module directly:

```rust
use noxapi::node_discovery;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Discover gateway from a domain
    let gateway = node_discovery::find_node_gateway("example.com").await?;
    println!("Discovered gateway: {}", gateway);
    
    // With explicit port
    let gateway = node_discovery::find_node_gateway("example.com:3042").await?;
    
    // IP address
    let gateway = node_discovery::find_node_gateway("192.168.1.100").await?;
    
    Ok(())
}
```

## Gateway Discovery Protocol

The library implements the Nox node discovery protocol with multiple fallback strategies:

### 1. Direct Connection (IP/localhost)

If the address is an IP address or localhost, the library tries to connect directly via HTTP.

```rust
// Direct connection to IP
let gateway = node_discovery::find_node_gateway("192.168.1.100").await?;
```

### 2. .well-known Endpoint

For domain names, the library first tries the `/.well-known/nox` endpoint:

- First attempts HTTPS: `https://example.com:3042/.well-known/nox`
- Falls back to HTTP: `http://example.com:3042/.well-known/nox`

```rust
// Will check .well-known endpoint first
let gateway = node_discovery::find_node_gateway("example.com").await?;
```

### 3. DNS TXT Records

If the .well-known endpoint fails, the library queries DNS TXT records via Google DNS API:

```
_nox.example.com TXT "mg=https://gateway.example.com"
```

The TXT record should contain a `mg=` (master gateway) field with the gateway URL.

Example DNS configuration:

```
_nox.example.com. 300 IN TXT "mg=https://gateway.example.com"
```

### 4. Fallback

If all discovery methods fail, the library returns the original address with HTTP protocol and default port (3042).

## Configuration

### Default Port

The default port for gateway discovery is `3042`. You can override it by specifying the port explicitly:

```rust
let gateway = node_discovery::find_node_gateway("example.com:8080").await?;
```

### Timeout

DNS and HTTP requests have a 5-second timeout by default. This is configured in the `node_discovery` module.

## API Methods

### `Nox::new(base_url)`

Create a new Nox client with a known gateway URL.

### `Nox::with_discovery(address)`

Create a new Nox client with automatic gateway discovery.

### `node_discovery::find_node_gateway(address)`

Discover the gateway URL from an address (domain, IP, or URL).

### `client.get_user_by_username(username)`

Get user information by username.

### `client.get_instance_by_id(id)`

Get instance information by ID.

## Error Handling

All async methods return `anyhow::Result` for easy error handling:

```rust
match Nox::with_discovery("example.com").await {
    Ok(client) => {
        // Use the client
    }
    Err(e) => {
        eprintln!("Gateway discovery failed: {}", e);
    }
}
```

## Examples

Check the `examples/` directory for more usage examples.

## License

AGPL-3.0
