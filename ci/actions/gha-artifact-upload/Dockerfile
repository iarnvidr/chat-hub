FROM golang:1.27-alpine AS build
WORKDIR /src
COPY main.go main_test.go go.mod ./
RUN go test ./...
RUN CGO_ENABLED=0 go build -trimpath -ldflags='-s -w' -o /artifact-uploader .

FROM alpine:3.22
RUN apk add --no-cache ca-certificates
COPY --from=build /artifact-uploader /usr/local/bin/artifact-uploader
ENTRYPOINT ["/usr/local/bin/artifact-uploader"]
