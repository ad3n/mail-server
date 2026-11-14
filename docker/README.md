# Docker Compose dengan PostgreSQL

Jalankan perintah berikut dari root repositori. Gunakan Docker Engine dengan Docker
Compose v2 atau lebih baru (`docker-compose` dapat dipakai jika tersedia sebagai
perintah terpisah). Image Stalwart dibangun dari checkout ini agar format
konfigurasi JSON sesuai dengan kode, sehingga build pertama dapat cukup lama.
Compose secara default memakai fitur `sqlite postgres enterprise` agar cocok
dengan edisi AGPL3/SEL dari Dockerfile awal. Edisi binary harus sama dengan edisi
yang membuat database. Image juga membangun Web UI dari revisi upstream yang
dipatok dengan overlay `resources/webui/src/`, termasuk menu integrasi Apps.
Build Compose menonaktifkan LTO, memakai 16 codegen units, dan membatasi build
ke 2 job untuk mengurangi kebutuhan RAM saat build lokal. Dockerfile standalone tetap
memakai LTO penuh secara default; opsi ini dapat diatur melalui build args
`STALWART_LTO`, `STALWART_CODEGEN_UNITS`, dan `STALWART_BUILD_JOBS`. Pada VM Docker dengan RAM terbatas,
siapkan RAM/swap yang cukup: build binary masih dapat kehabisan memori pada
VM 6 GB tanpa swap meskipun LTO dinonaktifkan.

## Memperbarui instalasi yang sudah memakai Web UI upstream

Pertahankan edisi database saat rebuild. Build dengan fitur `enterprise` memakai
edisi AGPL3/SEL, sedangkan build tanpa fitur tersebut memakai AGPL3-only; Stalwart
menolak membuka database dari edisi lain. Mengganti Web UI tidak memerlukan
perubahan edisi backend.

Jika database dibuat oleh Compose awal atau muncul error
`The data store was created by the Stalwart AGPL3/SEL edition`, gunakan di `.env`:

```dotenv
STALWART_FEATURES="sqlite postgres enterprise"
```

Jika database dibuat oleh Compose sebelumnya yang memakai `sqlite postgres`
tanpa `enterprise`, pertahankan edisi tersebut secara eksplisit:

```dotenv
STALWART_FEATURES="sqlite postgres"
```

Jangan menimpa `.env` yang sudah ada dengan `.env.example`. Cadangkan database
sebelum update sesuai bagian Backup. Jika layanan sedang berulang kali gagal
startup karena edisi, hentikan hanya service Stalwart dengan
`docker compose stop stalwart`, sesuaikan fitur, lalu rebuild:

```sh
docker compose up -d --build
docker compose logs --tail=100 stalwart
```

Tidak perlu menghapus data, menjalankan setup ulang, atau mengubah penanda edisi
di database. Bind mount yang persisten menjaga file tetap ada, tetapi tidak
menjamin kompatibilitas antar-edisi atau antar-versi binary.

Image menetapkan `STALWART_WEBUI_URL=file:///usr/local/share/stalwart/webui.zip`.
Server otomatis memakai bundle tersebut untuk aplikasi yang melayani `/admin`,
termasuk pada database yang sudah terisi. Bundle dibaca dari image setiap startup
atau reload aplikasi tanpa menggunakan cache bundle database, sehingga rebuild
langsung memuat perubahan lokal. Tidak perlu mengubah Resource URL atau menjalankan
Update Apps. Reload tab browser yang sudah terbuka setelah container aktif.

Menu Apps dapat digunakan dari `/admin/Management/x:App` maupun
`/account/Management/x:App` oleh admin global dengan izin
`sysSystemSettingsUpdate`. Keduanya harus merupakan prefix aplikasi Web UI yang
sama yang melayani `/admin`. Token login harus cocok dengan prefix halaman yang
dibuka; login ulang jika berpindah pintu masuk. Pemeriksaan origin dan pembatasan
izin tetap berlaku, sehingga pengguna biasa tidak mendapat akses pengelolaan
Apps. Perubahan backend ini memerlukan `docker compose up -d --build`.

Override ini tidak mengubah URL atau cache aplikasi yang sudah tersimpan di
database, maupun aplikasi lain yang tidak melayani `/admin`. Jika variabel
dihapus/dikosongkan, URL tersimpan kembali digunakan. Jangan hapus database atau
direktori konfigurasi untuk mengganti Web UI. File bundle yang hilang/rusak akan
dilaporkan sebagai error, bukan diam-diam diganti dengan Web UI upstream.

## Menjalankan instalasi baru

```sh
cp .env.example .env
openssl rand -hex 32
```

Isi `POSTGRES_PASSWORD` di `.env` dengan password hasil perintah tersebut, lalu
ubah `STALWART_HOSTNAME` ke hostname mail server Anda.

```sh
chmod 600 .env
docker compose up -d --build
docker compose logs stalwart
```

PostgreSQL hanya diakses lewat jaringan internal Compose. Service `init`
menyiapkan kepemilikan direktori Stalwart untuk UID/GID `2000:2000` sesuai
Dockerfile; Stalwart berjalan sebagai pengguna non-root. Jangan gunakan direktori
database lama dari PostgreSQL major version lain tanpa migrasi.

## Setup pertama

Buka <http://localhost:8080/admin> dan gunakan akun admin sementara yang muncul di
log Stalwart. Jika Docker berjalan di server remote, gunakan SSH tunnel:

```sh
ssh -L 8080:127.0.0.1:8080 user@server
```

Selesaikan wizard dengan memilih **PostgreSQL** sebagai **data store** (default
wizard masih RocksDB), lalu isi:

| Pengaturan | Nilai |
| --- | --- |
| Host | `postgres` |
| Port | `5432` |
| Database | `stalwart` |
| Username | `stalwart` |
| Password | Nilai `POSTGRES_PASSWORD` dari `.env` |
| TLS koneksi database | Nonaktif untuk jaringan internal Compose ini |
| Blob store | Default / gunakan data store |
| Search store | Default / gunakan data store |
| Directory | Internal |

Jika editor secret menyediakan tipe `EnvironmentVariable`, gunakan nama
`POSTGRES_PASSWORD`; variabel ini sudah diteruskan ke container Stalwart.
Gunakan path log `/var/log/stalwart/` dan path data lokal di bawah
`/var/lib/stalwart/`. Sesuaikan domain, akun administrator permanen, DNS, dan TLS
di wizard. Permintaan sertifikat publik memerlukan domain/DNS yang sudah benar;
nonaktifkan permintaan tersebut untuk percobaan lokal.

Wizard wajib dijalankan sekali: membuat `config.json` saja tidak menginisialisasi
akun, domain, dan listener. Sesudah setup gunakan `https://HOSTNAME/admin`.
Port 8080 digunakan untuk bootstrap/recovery; listener mail dan HTTPS dibuat
oleh setup. Mapping port Compose saja tidak mengaktifkan listener.

### Error login: Client metadata documents are disabled

Untuk **instalasi baru yang belum selesai setup**, gunakan SSH tunnel dan akses
`http://localhost:8080/admin/` seperti langkah setup di atas. Login dengan akun
sementara dari log; wizard baru ditampilkan setelah login. Mode bootstrap
memeriksa origin berdasarkan koneksi langsung ke Stalwart. Jika Nginx menerima
HTTPS lalu meneruskan HTTP, client HTTPS tidak cocok dengan origin HTTP tersebut.
Header `X-Forwarded-Proto` dan `STALWART_PUBLIC_URL` tidak mengatasi pemeriksaan
same-origin pada mode bootstrap ini. Selesaikan setup melalui tunnel, lalu gunakan
domain HTTPS publik untuk operasi normal.

Log `server.bootstrap-mode` dengan pesan `No configuration file was found`
menandakan server masuk mode setup. Jika `docker/config/config.json` masih ada,
server mencoba membuka data store yang dikonfigurasi; rebuild atau membuat ulang
container tidak menjadikan bind mount lama sebagai instalasi baru. Jangan hapus
config atau data hanya untuk mengatasi error login.

Untuk **instalasi yang sudah selesai setup**, periksa URL publik berikut.
Admin lokal dikenali berdasarkan domain dan port HTTPS yang dipercaya server.
Jika alamat browser berbeda dari hostname server (misalnya memakai IP, domain
reverse proxy, atau port HTTPS tambahan), login bisa dianggap sebagai client
eksternal dan ditolak dengan pesan ini.

Isi origin publik yang benar di `.env`, tanpa path `/admin`:

```dotenv
STALWART_PUBLIC_URL=https://mail.example.com
```

Gunakan alamat HTTPS yang benar-benar Anda buka; sertakan port jika bukan 443,
misalnya `https://mail.example.com:8443`. Lalu terapkan environment baru:

```sh
docker compose up -d
```

Buka kembali halaman `/admin/` pada alamat tersebut untuk memulai login baru.
`docker compose restart` saja tidak memperbarui environment container. Variabel
ini tidak mengubah isi database atau file konfigurasi. Jika kosong, hostname
server tetap digunakan. Tidak perlu mengaktifkan client metadata eksternal
untuk login ke admin lokal.

## Tes email dan webhook dengan Go

Isi konstanta `appKey` dan `appToken` di `mail.go` dengan kredensial dari menu Apps.
Pastikan URL webhook App sudah dikonfigurasi dan dapat dijangkau server.
Jalankan `go run mail.go` setelah mengisi environment `SMTP_PASS`. Opsi environment
lainnya adalah `SMTP_HOST`, `SMTP_PORT`, `SMTP_USER`, `MAIL_FROM`, `MAIL_TO`, dan
`EXTERNAL_ID`. Jika External-ID tidak diisi, script membuat ID otomatis.

`mail.go` mengecek API tracking otomatis setelah email diterima SMTP.
Script memakai App-Key/App-Token yang sama dengan header email dan menunggu semua
hasil berstatus `accepted`, `failed`, atau `removed`, lalu mencetak hasil JSON.
`queued`, `deferred`, dan HTTP 404 diperiksa ulang setiap 2 detik. Default batas
waktu 120 detik; atur `TRACKING_TIMEOUT=5m` untuk memperpanjangnya.
API default adalah `https://$SMTP_HOST` (port HTTPS standar), atau dapat diatur
melalui `TRACKING_API_URL=https://mail.moklaar.cloud`. Sertifikat diverifikasi dan
redirect API tidak diikuti. Error, timeout, atau hasil `failed`/`removed` memberi
exit code 1. Jangan mengirim ulang email hanya untuk mengecek status setelah timeout.

Status `accepted` berarti pengiriman selesai dan diterima tujuan, bukan bukti
masuk Inbox, dibaca, atau callback webhook diterima. Cocokkan `externalId` pada
hasil tracking dengan payload di endpoint webhook untuk memverifikasi callback.

## Lokasi data di host

| Direktori | Isi |
| --- | --- |
| `docker/config/` | `config.json`, koneksi data store yang ditulis wizard |
| `docker/postgres/` | Data PostgreSQL persisten |
| `docker/data/` | Data lokal tambahan Stalwart |
| `docker/logs/` | Log Stalwart |

Konfigurasi operasional, akun, email/blob, dan indeks pencarian berada di
PostgreSQL jika pilihan storage di atas dipertahankan. Backup `config.json`
saja **tidak cukup**. Direktori runtime, `.env`, dan `backups/` diabaikan Git.
`docker compose down` tidak menghapus bind mount ini.

## Unbound lokal dengan DNSSEC

Gunakan overlay `compose.unbound.yaml` bersama Compose utama. Unbound melakukan
resolusi rekursif langsung, tanpa forwarder Google/Cloudflare. Port 53 tidak
dipublikasikan ke host. ACL hanya menerima localhost dan subnet resolver Docker.
DNSSEC memakai validator dan root trust anchor dari paket `dns-root-data`, lalu
memperbaruinya melalui RFC 5011 di volume `unbound-state` yang persisten.

Pastikan subnet `172.30.53.0/24` tidak bentrok dengan jaringan server/VPN. Jika
bentrok, sesuaikan subnet dan IP di overlay, ACL di `docker/unbound/unbound.conf`,
serta alamat resolver di Web UI. Server harus dapat mengakses authoritative DNS
langsung melalui UDP dan TCP port 53, dan jam host harus tersinkronisasi.

Mulai Unbound dahulu; resolver Stalwart yang sekarang tetap berlaku:

```sh
docker compose -f compose.yaml -f compose.unbound.yaml up -d --build unbound
docker compose -f compose.yaml -f compose.unbound.yaml exec unbound unbound-checkconf
docker compose -f compose.yaml -f compose.unbound.yaml exec unbound dig @127.0.0.1 . DNSKEY +dnssec
docker compose -f compose.yaml -f compose.unbound.yaml exec unbound dig @127.0.0.1 dnssec-failed.org A +dnssec
docker compose -f compose.yaml -f compose.unbound.yaml exec unbound dig @127.0.0.1 moklaar.co.id MX +dnssec
docker compose -f compose.yaml -f compose.unbound.yaml exec unbound dig @127.0.0.1 moklaar.co.id DS +dnssec +tcp
```

Hasil yang diharapkan: root DNSKEY `NOERROR` dengan flag `ad`; domain uji
`dnssec-failed.org` menghasilkan `SERVFAIL`; MX moklaar berhasil (tanpa `ad`
karena unsigned); DS moklaar `NOERROR`, tanpa jawaban DS, dengan flag `ad`.
`SERVFAIL` saja tidak membuktikan validator bekerja jika query valid juga gagal.
Jika gagal, periksa `logs --tail=100 unbound`, koneksi keluar port 53, dan jam host.

Hubungkan container Stalwart ke jaringan resolver. Perintah ini dapat membuat
ulang container Stalwart, sehingga jalankan pada waktu yang sesuai untuk restart:

```sh
docker compose -f compose.yaml -f compose.unbound.yaml up -d --no-build stalwart
```

Di Web UI, buka **Settings → Network → DNS → DNS Resolver**, pilih **Custom**:

| Address | Port | Protocol |
| --- | --- | --- |
| `172.30.53.53` | `53` | `udp` |
| `172.30.53.53` | `53` | `tcp` |

Aktifkan **Enable EDNS** dan **TCP on error**; pertahankan opsi lain pada default.
TCP harus menjadi entri server tersendiri agar pemeriksaan kemampuan DNSSEC/DANE
Stalwart tersedia. TLS tidak diperlukan untuk koneksi di bridge Docker ini.
Jangan memakai `127.0.0.1`: alamat itu menunjuk container Stalwart sendiri.
Simpan, jalankan **Reload Settings** jika tersedia, kemudian ulangi Tester.
Jika konfigurasi belum diterapkan, restart service Stalwart dengan kedua file
Compose tersebut. Verifikasi log startup tidak menunjukkan DANE dinonaktifkan.

Untuk memeriksa akses Spamhaus public mirror (sesuai ketentuan akun/penggunaan):

```sh
docker compose -f compose.yaml -f compose.unbound.yaml exec unbound dig @127.0.0.1 2.0.0.127.zen.spamhaus.org A
```

Alamat uji harus menghasilkan kode listing yang didokumentasikan Spamhaus.
Jawaban `127.255.255.*` adalah error akses, bukan listing spam. Resolver sendiri
tidak menghapus batas penggunaan atau kebutuhan langganan penyedia DNSBL.
Tes DNS ini tetap perlu dilengkapi pemeriksaan hasil filter spam Stalwart.

Selanjutnya selalu gunakan **kedua file Compose** untuk perintah `up`, `build`,
dan `down`, agar koneksi jaringan resolver tidak hilang. Untuk rollback, ubah
resolver Web UI kembali ke Google dan terapkan sebelum menghentikan Unbound.
Rebuild image Unbound berkala dengan `build --pull --no-cache unbound`, kemudian
`up -d unbound` (keduanya dengan dua file Compose) untuk pembaruan paket keamanan.
Jangan menghapus volume `unbound-state` pada pemeliharaan rutin.

Referensi: [resolver Stalwart](https://www.stalw.art/docs/server/dns/resolver/),
[konfigurasi Unbound](https://unbound.docs.nlnetlabs.nl/en/latest/manpages/unbound.conf.html).

## Backup

Hentikan Stalwart sementara agar dump database dan file lokal satu keadaan.
PostgreSQL tetap berjalan untuk `pg_dump`. Jalankan berurutan, dan lanjutkan hanya
jika setiap perintah berhasil:

```sh
mkdir -p backups
chmod 700 backups
backup_dir="backups/$(date +%Y%m%d-%H%M%S)"
mkdir -m 700 "$backup_dir"
docker compose stop stalwart
docker compose exec -T postgres sh -ec 'pg_dump -U "$POSTGRES_USER" -d "$POSTGRES_DB" -Fc' > "$backup_dir/postgres.dump"
sudo tar -czf "$backup_dir/files.tar.gz" .env compose.yaml docker/config docker/data docker/logs
docker compose start stalwart
```

Jika backup gagal, jalankan `docker compose start stalwart` untuk memulihkan
layanan dan jangan gunakan backup yang tidak lengkap. Simpan kedua file backup
di lokasi lain yang aman; arsip berisi kredensial. Catat juga commit/image
Stalwart dan versi PostgreSQL untuk restore. Jangan menyalin langsung
`docker/postgres/` ketika database sedang berjalan.

## Restore ke instalasi baru

Gunakan checkout/image Stalwart yang sama dan PostgreSQL major version 17.
Pastikan layanan Stalwart berhenti dan target database baru/kosong; langkah ini
ditujukan untuk pemulihan ke direktori baru, bukan menimpa instalasi aktif.
Ganti `BACKUP` dengan path backup:

```sh
sudo tar -xzf BACKUP/files.tar.gz
docker compose up -d --wait postgres
docker compose exec -T postgres sh -ec 'pg_restore --exit-on-error --no-owner -U "$POSTGRES_USER" -d "$POSTGRES_DB"' < BACKUP/postgres.dump
docker compose up -d --build
docker compose logs --tail=100 stalwart
```

Pertahankan password `.env` yang sesuai dengan database dan secret konfigurasi.
Mengubah `.env` tidak otomatis mengganti password role PostgreSQL pada database
yang sudah terinisialisasi. Jika storage tambahan dikonfigurasi di luar mount
ini, cadangkan dan pulihkan storage tersebut juga.
