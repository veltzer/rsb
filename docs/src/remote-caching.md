# Remote Caching

RSConstruct supports sharing build artifacts across machines via remote caching. When enabled, build outputs are pushed to a remote store and can be pulled by other machines, avoiding redundant rebuilds.

## Configuration

Add a `remote` URL to your `[cache]` section in `rsconstruct.toml`:

```toml
[cache]
remote = "s3://my-bucket/rsconstruct-cache"
```

## Supported Backends

### Amazon S3

```toml
[cache]
remote = "s3://bucket-name/optional/prefix"
```

Requires:
- AWS CLI installed (`aws` command)
- AWS credentials configured (`~/.aws/credentials` or environment variables)

The S3 backend uses `aws s3 cp` and `aws s3 ls` commands.

### HTTP/HTTPS

```toml
[cache]
remote = "http://cache-server.example.com:8080/rsconstruct"
# or
remote = "https://cache-server.example.com/rsconstruct"
```

Requires:
- `curl` command
- Server that supports GET and PUT requests

The HTTP backend expects:
- `GET /path` to return the object or 404
- `PUT /path` to store the object
- `HEAD /path` to check existence (returns 200 or 404)

### Local Filesystem

```toml
[cache]
remote = "file:///shared/cache/rsconstruct"
```

Useful for:
- Network-mounted filesystems (NFS, CIFS)
- Testing remote cache behavior locally

## Control Options

You can control push and pull separately:

```toml
[cache]
remote = "s3://my-bucket/rsconstruct-cache"
remote_push = true   # Push local builds to remote (default: true)
remote_pull = true   # Pull from remote on cache miss (default: true)
```

### Pull-only mode

To share a read-only cache (e.g., from CI):

```toml
[cache]
remote = "s3://ci-cache/rsconstruct"
remote_push = false
remote_pull = true
```

### Push-only mode

To populate a cache without using it (e.g., in CI):

```toml
[cache]
remote = "s3://ci-cache/rsconstruct"
remote_push = true
remote_pull = false
```

## How It Works

### Cache Structure

Remote cache stores two types of objects, mirroring the local layout:

1. **Descriptors** at `descriptors/{descriptor_key}`
   - JSON describing what a product build produced: a marker (a check
     passed), a blob (one output), or a tree (a list of output paths and
     their content checksums)
   - One entry per product build — see [Cache keys](internal/cache.md#cache-keys)

2. **Objects** at `objects/{xx}/{rest_of_checksum}`
   - Content-addressed storage (like git)
   - Actual file contents identified by SHA-256

### On Build

1. RSConstruct computes the cache key and input checksum
2. Checks local cache first
3. If local miss and `remote_pull = true`:
   - Fetches the descriptor from remote (and caches it locally)
   - Fetches the objects the descriptor names
   - Restores outputs locally

   Fetched objects are verified against their checksum before being admitted
   to the local content-addressed store, and fetched descriptors must parse;
   a corrupt remote cannot poison the local cache. A remote miss, or an
   unreachable remote, simply degrades to a local rebuild.
4. If rebuild required:
   - Executes the processor
   - Stores outputs in local cache
   - If `remote_push = true`, pushes the objects and the descriptor to remote

### Cache Hit Flow

```
Local cache hit → Restore from local → Done
       ↓ miss
Remote cache hit → Download descriptor + objects → Restore → Done
       ↓ miss
Execute processor → Cache locally → Push to remote → Done
```

## Best Practices

### CI/CD Integration

In your CI pipeline:

```yaml
# .github/workflows/build.yml
env:
  AWS_ACCESS_KEY_ID: ${{ secrets.AWS_ACCESS_KEY_ID }}
  AWS_SECRET_ACCESS_KEY: ${{ secrets.AWS_SECRET_ACCESS_KEY }}

steps:
  - run: rsconstruct build
```

### Separate CI and Developer Caches

Use different prefixes to avoid conflicts:

```toml
# CI: rsconstruct.toml.ci
[cache]
remote = "s3://cache/rsconstruct/ci"
remote_push = true
remote_pull = true
```

```toml
# Developers: rsconstruct.toml
[cache]
remote = "s3://cache/rsconstruct/ci"
remote_push = false  # Read from CI cache only
remote_pull = true
```

### Cache Invalidation

Cache entries are keyed by:
- Processor name
- Source file path
- Processor configuration hash

To force a full rebuild ignoring caches:

```bash
rsconstruct build --force
```

To clear only the local cache:

```bash
rsconstruct cache clear
```

## Troubleshooting

### S3 Access Denied

Check your AWS credentials:

```bash
aws s3 ls s3://your-bucket/
```

### HTTP Upload Failures

Ensure your server accepts PUT requests. Many static file servers are read-only.

### Slow Remote Cache

Consider:
- Using a closer region for S3
- Enabling S3 Transfer Acceleration
- Using a caching proxy

### Debug Mode

Use verbose output to see cache operations:

```bash
rsconstruct build -v
```

This shows which products are restored from local cache, remote cache, or rebuilt.
