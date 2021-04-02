# warp-tracing-playground

## Start Jaeger

```
docker run \
 -p 6831:6831/udp \
 -p 6832:6832/udp \
 -p 16686:16686 \
 -p 14268:14268 \
 jaegertracing/all-in-one:latest
```

## Start Service w/ Log Parser

```
cargo r --quiet | npx bunyan
```

## Create Traffic

```
cat << EOF | http POST localhost:3030/users
{
  "firstName": "Jane",
  "lastName": "Doe",
  "gender": "female",
  "id": 123
}
EOF
```

```
http localhost:3030/users
```

## Check Metrics

```
http localhost:3030/metrics
```

## Check Traces

```
open http://localhost:16686
```
