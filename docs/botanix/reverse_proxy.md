
### Summary
This diagram represents the Botanix web architecture hosted on Google Compute Engine and served through an Nginx reverse proxy. The various parts of the application is accessed through various subdomains, which are routed through Cloudflare's reverse proxy for added security and performance. The Google Compute Engine hosts Nginx for proxying requests and multiple Docker containers that run different components of the application, including Node.js, Sidecar, the DApp itself, and a Bitcoin server. The diagram illustrates the connections between these components and how they interact to serve the DApp to users.


### Diagram

```mermaid  
graph TD;

subgraph Internet
  A[DApp]
end

subgraph Cloudflare Reverse Proxy
  B[bridge.botanixlabs.dev]
  C[node.botanixlabs.dev]
  D[sidecar.botanixlabs.dev]
end

subgraph Google Compute Engine
    subgraph Ngnix Reverse Proxy
        E[Ngnix]
    end
    subgraph Docker Containers
        F[Node]
        G[Sidecar]
        H[Dapp]
        K[Btc Server]
        L[Node]
    end
end

A --> B;
A --> C;
A --> D;

B --> E;
C --> E;
D --> E;

E --> F;
E --> G;
E --> H;
E --> K;
E --> L;

```
