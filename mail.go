package main

import (
	"context"
	"crypto/rand"
	"crypto/tls"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"net/smtp"
	"net/url"
	"os"
	"strconv"
	"strings"
	"time"
)

const (
	appKey   = "nKn0VC6hlhCm8Tq7GC8Ad92d21CbPKgv"
	appToken = "6Lrh3XnWBDu4aFGORnunKdbHWoAg5Xir"
)

type deliveryStatus struct {
	EventID    string `json:"eventId"`
	AppID      string `json:"appId"`
	ExternalID string `json:"externalId"`
	QueueID    string `json:"queueId"`
	Recipient  string `json:"recipient"`
	Status     string `json:"status"`
	CreatedAt  string `json:"createdAt"`
	UpdatedAt  string `json:"updatedAt"`
}

func trackingURL(origin, externalID string) (string, error) {
	base, err := url.Parse(origin)
	if err != nil {
		return "", fmt.Errorf("TRACKING_API_URL tidak valid")
	}

	if base.Scheme != "https" || base.Hostname() == "" || base.User != nil || base.RawQuery != "" || base.Fragment != "" || (base.Path != "" && base.Path != "/") {
		return "", fmt.Errorf("TRACKING_API_URL harus origin HTTPS tanpa path, user, query, atau fragment")
	}

	base.Path = "/api/deliveries/" + externalID + "/status"
	return base.String(), nil
}

func trackingClient() *http.Client {
	return &http.Client{
		Timeout: 15 * time.Second,
		CheckRedirect: func(_ *http.Request, _ []*http.Request) error {
			return http.ErrUseLastResponse
		},
	}
}

func fetchTracking(ctx context.Context, client *http.Client, endpoint string) ([]deliveryStatus, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, endpoint, nil)
	if err != nil {
		return nil, err
	}

	req.Header.Set("App-Key", appKey)
	req.Header.Set("App-Token", appToken)
	response, err := client.Do(req)
	if err != nil {
		return nil, err
	}
	defer response.Body.Close()

	if response.StatusCode == http.StatusNotFound {
		return nil, nil
	}

	if response.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("API tracking HTTP %d", response.StatusCode)
	}

	body, err := io.ReadAll(io.LimitReader(response.Body, (1<<20)+1))
	if err != nil {
		return nil, err
	}

	if len(body) > 1<<20 {
		return nil, fmt.Errorf("respons API tracking terlalu besar")
	}

	var rows []deliveryStatus
	if err := json.Unmarshal(body, &rows); err != nil {
		return nil, fmt.Errorf("respons API tracking tidak valid: %w", err)
	}

	return rows, nil
}

func waitTracking(ctx context.Context, client *http.Client, endpoint string, interval time.Duration) ([]deliveryStatus, error) {
	for {
		rows, err := fetchTracking(ctx, client, endpoint)
		if err != nil {
			return nil, err
		}

		final := len(rows) > 0
		failed := false
		for _, row := range rows {
			switch row.Status {
			case "accepted":
			case "failed", "removed":
				failed = true
			case "queued", "deferred":
				final = false
			default:
				return rows, fmt.Errorf("status tracking tidak dikenal: %q", row.Status)
			}
		}

		if final {
			if failed {
				return rows, fmt.Errorf("pengiriman berakhir dengan status failed/removed")
			}

			return rows, nil
		}

		timer := time.NewTimer(interval)
		select {
		case <-ctx.Done():
			timer.Stop()
			return rows, ctx.Err()
		case <-timer.C:
		}
	}
}

func getEnv(name, fallback string) string {
	value := os.Getenv(name)
	if value == "" {
		return fallback
	}

	return value
}

func main() {
	smtpHost := getEnv("SMTP_HOST", "mail.moklaar.cloud")
	smtpPort := getEnv("SMTP_PORT", "587")
	smtpUser := getEnv("SMTP_USER", "admin@mail.moklaar.cloud")
	smtpPass := os.Getenv("SMTP_PASS")
	mailFrom := getEnv("MAIL_FROM", smtpUser)
	mailTo := getEnv("MAIL_TO", "surya.kejawen@gmail.com")

	if smtpPass == "" {
		log.Fatal("SMTP_PASS belum diisi")
	}

	port, err := strconv.Atoi(smtpPort)
	if err != nil {
		log.Fatalf("SMTP_PORT tidak valid: %v", err)
	}

	if appKey == "ISI_APP_KEY" || appToken == "ISI_APP_TOKEN" || appKey == "" || appToken == "" {
		log.Fatal("Isi konstanta appKey dan appToken dengan kredensial dari menu Apps")
	}

	if strings.ContainsAny(appKey+appToken, "\r\n") {
		log.Fatal("App-Key dan App-Token tidak boleh mengandung baris baru")
	}

	externalID := os.Getenv("EXTERNAL_ID")
	if externalID == "" {
		var randomID [16]byte
		if _, err := rand.Read(randomID[:]); err != nil {
			log.Fatalf("Gagal membuat External-ID: %v", err)
		}

		externalID = hex.EncodeToString(randomID[:])
	}

	if len(externalID) > 128 || externalID == "." || externalID == ".." || strings.IndexFunc(externalID, func(r rune) bool {
		return !(r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' || r == '-' || r == '_' || r == '.')
	}) >= 0 {
		log.Fatal("EXTERNAL_ID maksimal 128 karakter: huruf ASCII, angka, tanda -, _, atau .; bukan . atau ..")
	}

	endpoint, err := trackingURL(getEnv("TRACKING_API_URL", "https://"+smtpHost), externalID)
	if err != nil {
		log.Fatal(err)
	}

	wait, err := time.ParseDuration(getEnv("TRACKING_TIMEOUT", "120s"))
	if err != nil || wait <= 0 {
		log.Fatal("TRACKING_TIMEOUT harus durasi positif, misalnya 120s atau 5m")
	}

	message := fmt.Sprintf(
		"Date: %s\r\n"+
			"From: %s\r\n"+
			"To: %s\r\n"+
			"Subject: Test webhook Stalwart\r\n"+
			"App-Key: %s\r\n"+
			"App-Token: %s\r\n"+
			"External-ID: %s\r\n"+
			"MIME-Version: 1.0\r\n"+
			"Content-Type: text/plain; charset=UTF-8\r\n"+
			"\r\n"+
			"Email ini dikirim untuk mengetes custom header dan webhook Stalwart.\r\n",
		time.Now().Format(time.RFC1123Z),
		mailFrom,
		mailTo,
		appKey,
		appToken,
		externalID,
	)

	address := net.JoinHostPort(smtpHost, strconv.Itoa(port))

	conn, err := net.Dial("tcp", address)
	if err != nil {
		log.Fatalf("Gagal terhubung ke SMTP: %v", err)
	}
	defer conn.Close()

	client, err := smtp.NewClient(conn, smtpHost)
	if err != nil {
		log.Fatalf("Gagal membuat SMTP client: %v", err)
	}
	defer client.Close()

	tlsConfig := &tls.Config{
		ServerName: smtpHost,
		MinVersion: tls.VersionTLS12,
	}

	if ok, _ := client.Extension("STARTTLS"); !ok {
		log.Fatal("Server SMTP tidak mendukung STARTTLS")
	}

	if err := client.StartTLS(tlsConfig); err != nil {
		log.Fatalf("STARTTLS gagal: %v", err)
	}

	auth := smtp.PlainAuth("", smtpUser, smtpPass, smtpHost)

	if err := client.Auth(auth); err != nil {
		log.Fatalf("Autentikasi SMTP gagal: %v", err)
	}

	if err := client.Mail(mailFrom); err != nil {
		log.Fatalf("MAIL FROM gagal: %v", err)
	}

	if err := client.Rcpt(mailTo); err != nil {
		log.Fatalf("RCPT TO gagal: %v", err)
	}

	writer, err := client.Data()
	if err != nil {
		log.Fatalf("Membuka DATA SMTP gagal: %v", err)
	}

	if _, err := writer.Write([]byte(message)); err != nil {
		log.Fatalf("Gagal menulis isi email: %v", err)
	}

	if err := writer.Close(); err != nil {
		log.Fatalf("Gagal menutup DATA SMTP: %v", err)
	}

	if err := client.Quit(); err != nil {
		log.Printf("SMTP QUIT gagal setelah DATA diterima; tetap periksa tracking: %v", err)
	}

	fmt.Println("Email diterima SMTP.")
	fmt.Printf("External-ID: %s\n", externalID)
	fmt.Printf("Menunggu status akhir (batas %s)...\n", wait)
	ctx, cancel := context.WithTimeout(context.Background(), wait)
	defer cancel()

	rows, err := waitTracking(ctx, trackingClient(), endpoint, 2*time.Second)
	if len(rows) > 0 {
		result, _ := json.MarshalIndent(rows, "", "  ")
		fmt.Println(string(result))
	}

	if err != nil {
		log.Fatalf("Pengecekan status: %v. External-ID: %s; email sudah diterima SMTP, jangan kirim ulang hanya untuk mengecek status.", err, externalID)
	}

	fmt.Println("ACCEPTED: semua hasil pengiriman diterima tujuan; bukan bukti email dibaca atau callback webhook diterima.")
}
