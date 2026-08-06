# printfd - printf daemon

daemon that prints files to a local ipp printer using server-sent events

## how it works

- loads config from local os directory
- continuously listens to the configured sse url
- parses incoming 'update' events for print attributes
- routes the job to the correct ipp printer based on color profiles
- spawns async tasks to print documents using ipp

## config

```json
{
    "eventUrl": "<your-event-url>",
    "s3BaseUrl": "<your-s3-base-url>"
}
```

## start

```bash
cargo run --release
``` 