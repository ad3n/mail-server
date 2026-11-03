# Mengakses API Stalwart

Impor `openapi.yml` ke Postman atau Swagger UI. Pilih server `http://localhost:8080` untuk instance lokal ini. Untuk deployment lain, gunakan origin HTTPS Stalwart. Di Swagger UI, gunakan **Authorize** sesuai operasi.

| Client | Endpoint | Autentikasi |
|---|---|---|
| Browser atau client OAuth | `GET /api/discover/{email}`, `POST /api/auth` | Discovery dan awal login dapat diakses tanpa token. Ikuti alur OAuth dan PKCE yang dijelaskan pada operasi login untuk mendapatkan access token. |
| Pengguna Stalwart | `GET /api/account`, `GET /api/schema`, `GET /api/schema/{hash}` | HTTP Basic atau `Authorization: Bearer <access_token>`. Gunakan `curl -L` untuk mengikuti redirect schema. |
| Aplikasi pengirim | `GET /api/deliveries/{externalId}/status` | Kirim **kedua** header `App-Key` dan `App-Token` milik aplikasi. Hasilnya hanya mencakup pengiriman aplikasi tersebut. |
| Client diagnostik | `GET /api/token/{jenis}` dan `GET /api/live/…` | Basic atau Bearer dengan izin terkait. Untuk EventSource, ambil token berumur pendek dari endpoint token yang sesuai dan kirim sebagai `?token=…` ke stream. Tracing dan metrics memerlukan Enterprise. |

## Administrasi aplikasi

Admin sistem mengelola aplikasi, webhook, serta kredensial melalui **Management → Directory → Apps** di `/admin/Management/x:IntegrationApp`.

## Pengiriman dan lookup

Saat submission SMTP terautentikasi, tambahkan header pesan `External-ID`, `App-Key`, dan `App-Token`. Stalwart memverifikasi kredensial aplikasi sebelum mengambil nilai External-ID. Jika kredensial tidak lengkap, tidak valid, atau tidak ditemukan, pengiriman dilanjutkan tanpa tracking dan tanpa validasi External-ID. App-Key dan App-Token dibersihkan sebelum pengiriman ke penerima. External-ID hanya diambil dan dihapus ketika tracking aktif, sehingga header tersebut pada email tanpa tracking tetap dipertahankan. Kredensial valid tetap memerlukan tepat satu External-ID yang valid. Gangguan database autentikasi menghasilkan kegagalan SMTP sementara agar client dapat retry. Untuk membaca status, isi variabel dari kredensial aplikasi dan gunakan External-ID yang sama persis, termasuk nol di depan.

```bash
BASE=http://localhost:8080
curl -i -H "App-Key: $APP_KEY" -H "App-Token: $APP_TOKEN" \
  "$BASE/api/deliveries/00042/status"
```

Pembacaan daftar Apps dan status dibatasi hingga 10.000 record dan total 16 MiB data tersimpan per request. Jika batas terlampaui, request gagal dengan error server tanpa mengembalikan hasil parsial. Setiap record tracking dibatasi 256 KiB JSON. Batas ini melindungi penggunaan memori dan belum menyediakan pagination.

HTTP 200 menghasilkan array status per penerima. HTTP 401 menunjukkan kredensial tidak cocok. HTTP 404 berarti tidak ada hasil yang dapat diakses untuk External-ID itu.

## Autentikasi webhook keluar

Stalwart mengirim satu POST JSON per perubahan status ke `webhookUrl`. Konfigurasi di bawah diisi admin sesuai kontrak **server penerima webhook**, terpisah dari kredensial lookup yang dihasilkan Stalwart.

| Auth Type | webhookAuth | webhookSecrets |
|---|---|---|
| None (default) | `{"type":"none"}` | Tidak diperlukan |
| Basic | `{"type":"basic","username":"billing"}` | `{"password":"…"}` |
| Bearer | `{"type":"bearer"}` | `{"token":"…"}` |
| API Key | `{"type":"apiKey","keyHeader":"X-Api-Key"}` | `{"apiKey":"…"}` |
| API Key + Token | `{"type":"apiKeyToken","keyHeader":"X-Api-Key","tokenHeader":"X-Webhook-Token"}` | `{"apiKey":"…","token":"…"}` |

Untuk Basic dan Bearer, Stalwart menyusun header Authorization. Untuk API Key dan API Key + Token, isi nama header dan nilai yang diharapkan receiver. Nilai rahasia hanya dapat ditulis. Saat mengedit dengan URL dan jenis autentikasi yang sama, nilai rahasia yang dikosongkan mempertahankan nilai tersimpan. Jika URL atau jenis autentikasi berubah, isi kembali semua rahasia yang dibutuhkan.
