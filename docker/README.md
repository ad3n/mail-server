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
