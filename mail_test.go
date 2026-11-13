package main

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"sync/atomic"
	"testing"
	"time"
)

func TestTrackingPollsUntilAccepted(t *testing.T) {
	var calls atomic.Int32
	responses := []string{``, `[{"status":"queued"}]`, `[{"status":"deferred"}]`, `[{"status":"accepted"}]`}
	server := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("App-Key") != appKey || r.Header.Get("App-Token") != appToken || r.URL.Path != "/api/deliveries/test-id/status" {
			t.Error("incorrect tracking request")
		}

		index := int(calls.Add(1) - 1)
		if index == 0 {
			w.WriteHeader(http.StatusNotFound)
			return
		}

		if index >= len(responses) {
			t.Error("polling continued after accepted")
			w.WriteHeader(http.StatusInternalServerError)
			return
		}

		_, _ = w.Write([]byte(responses[index]))
	}))
	defer server.Close()

	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()

	rows, err := waitTracking(ctx, server.Client(), server.URL+"/api/deliveries/test-id/status", time.Millisecond)
	if err != nil || len(rows) != 1 || rows[0].Status != "accepted" || calls.Load() != 4 {
		t.Fatalf("unexpected result: rows=%v calls=%d err=%v", rows, calls.Load(), err)
	}
}

func TestTrackingTerminalFailuresAndInvalidResponses(t *testing.T) {
	for _, body := range []string{`[{"status":"failed"}]`, `[{"status":"removed"}]`, `[{"status":"accepted"},{"status":"failed"}]`, `[{"status":"delivered"}]`, `{`, `{}`, `[{}]`} {
		t.Run(body, func(t *testing.T) {
			server := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
				_, _ = w.Write([]byte(body))
			}))
			defer server.Close()

			ctx, cancel := context.WithTimeout(context.Background(), time.Second)
			defer cancel()

			_, err := waitTracking(ctx, server.Client(), server.URL, time.Millisecond)
			if err == nil || errors.Is(err, context.DeadlineExceeded) {
				t.Fatalf("expected immediate failure, got %v", err)
			}
		})
	}
}

func TestTrackingPendingTimesOut(t *testing.T) {
	for _, body := range []string{`[]`, `[{"status":"accepted"},{"status":"queued"}]`, `[{"status":"deferred"}]`} {
		t.Run(body, func(t *testing.T) {
			server := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
				_, _ = w.Write([]byte(body))
			}))
			defer server.Close()

			ctx, cancel := context.WithTimeout(context.Background(), 50*time.Millisecond)
			defer cancel()

			_, err := waitTracking(ctx, server.Client(), server.URL, time.Millisecond)
			if !errors.Is(err, context.DeadlineExceeded) {
				t.Fatalf("expected timeout, got %v", err)
			}
		})
	}
}

func TestTrackingRejectsHTTPFailuresAndRedirects(t *testing.T) {
	for _, status := range []int{301, 302, 401, 403, 500} {
		var calls atomic.Int32
		server := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			calls.Add(1)
			w.Header().Set("Location", "/redirected")
			w.WriteHeader(status)
		}))
		client := trackingClient()
		client.Transport = server.Client().Transport
		_, err := fetchTracking(context.Background(), client, server.URL)
		server.Close()
		if err == nil || calls.Load() != 1 {
			t.Fatalf("status %d: expected failure without redirect", status)
		}
	}
}

func TestTrackingURL(t *testing.T) {
	endpoint, err := trackingURL("https://mail.example.test:8443/", "test-id")
	if err != nil || endpoint != "https://mail.example.test:8443/api/deliveries/test-id/status" {
		t.Fatalf("unexpected endpoint %q: %v", endpoint, err)
	}

	for _, origin := range []string{"http://mail.example.test", "https://user:pass@mail.example.test", "https://mail.example.test/admin", "https://mail.example.test?x=1", "https://mail.example.test#fragment"} {
		if _, err := trackingURL(origin, "test-id"); err == nil {
			t.Errorf("accepted invalid origin %q", origin)
		}
	}
}
