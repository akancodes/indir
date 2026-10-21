<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" width="112" height="112" alt="">
</p>

<h1 align="center">indir</h1>

<p align="center">
  <b>Windows, macOS ve Linux için küçük bir indirme yöneticisi.</b><br>
  Linki yapıştırın. indir dosyayı parçalara bölüp aynı anda birkaç bağlantıdan indirir, böylece daha çabuk gelir;<br>
  duraklatırsanız, bilgisayarı yeniden başlatsanız bile kaldığı yerden devam eder.
</p>

<p align="center">
  <a href="https://github.com/akancodes/indir/releases/latest"><b>İndir</b></a> ·
  Ücretsiz ve açık kaynak · Reklam yok, hesap yok, takip yok ·
  <a href="README.md">English</a>
</p>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/screenshot-dark.png">
  <img src="docs/screenshot-light.png" alt="Parça parça inen iki dosya, sırasını bekleyen bir dosya ve biten bir dosyayla, iş bitince bilgisayarı kapatmaya ayarlı indir">
</picture>

## Neden indir

Internet Download Manager'ı (IDM) kullandıysanız fikir aynı, geri kalan her şey çıkarılmış hâli. Tarayıcı bir
dosyayı tek bağlantıdan indirir; çoğu sunucu da tek bir bağlantının hızını sınırladığı için büyük dosyalar
sürünür. indir dosyayı parçalara böler, parçaları yan yana indirir, sonra birleştirir. Bir pencere, linki
yapıştıracağınız bir alan ve kısa bir ayarlar sayfası var, o kadar.

## Neler yapar

- **Parça parça indirir.** Her parça kendi bağlantısından gelir, varsayılan olarak 8 bağlantı. Bağlantı başına
  hız sınırı koyan bir CDN'de bu birkaç kat hız demek: 148 MB'lık bir Linux çekirdeği arşivi 8,6 saniye yerine
  2,6 saniyede indi.
- **Her bağlantıyı meşgul tutar.** Erken biten bir bağlantı, hâlâ inmekte olan en büyük parçanın yarısını
  devralır; böylece indirmenin sonu tek bir yavaş bağlantıyı beklemez. Susan ya da damla damla akan bir
  bağlantının parçasının tamamı devralınır.
- **Sunucu isterse geri çekilir.** Bazı sunucular çok bağlantı açanları yavaşlatır ya da fazlasını
  "429 Too Many Requests" ile geri çevirir. indir bunu fark eder, bir süre tek bağlantıyla dener ve hangisi
  hızlıysa onda kalır. Amacı hiçbir zaman tarayıcınızdan yavaş olmamak.
- **Duraklatır ve devam eder.** Bir indirmeyi durdurup sonra devam ettirebilirsiniz; indir'i kapatsanız ya da
  bilgisayarı yeniden başlatsanız bile. Sunucudaki dosya değişirse, indirme sürerken de olsa arada da olsa, indir
  bunu anlar ve iki sürümü karıştırmak yerine baştan başlar.
- **Kopan bağlantıya takılmaz.** Kopan ya da takılan bir bağlantı tam kaldığı yerden yeniden açılır. GitHub'ın
  sürüm indirmeleri gibi, süresi dolan imzalı bir adrese götüren linklere yeniden sorulur.
- **Sıra bekletir.** On link yapıştırın, üçü aynı anda iner; diğerleri sıralarını bekler ve kendiliğinden başlar.
  *Start now* biriyle sırayı atlatır, *Pause all* her şeyi durdurur.
- **Başkalarına da yer bırakır.** Bir hız sınırı bütün indirmeleri toplamda, diyelim saniyede 5 MB'ta tutar;
  evdeki diğerleri film izlemeye devam edebilir. Sınır hemen, sürmekte olan indirmelere de uygulanır.
- **İş bitince uyutur ya da kapatır.** İnecek bir şey kalmayınca bilgisayarı uyku moduna almasını, kapatmasını
  ya da kendisinin kapanmasını söyleyin. Önce bir dakika bekler; başındaysanız fikrinizi değiştirebilirsiniz.
  Sorun yüzünden duran bir indirme varsa bunu da söyler. Bir şey inerken bilgisayar kendiliğinden uykuya
  geçmez, ekran yine de kapanabilir.
- **Dosya hazır olunca haber verir;** indir önde değilse bir bildirimle.
- **Kopyalanan linki yakalar.** Tarayıcıda bir link kopyalayıp indir'e geçin: link alanda hazırdır, geriye
  Enter'a basmak kalır. Bir indirmeye sağ tıklayarak linkini kopyalayabilir, yeniden indirebilir ya da
  klasöründe gösterebilirsiniz.
- **Ne yaptığını gösterir.** Her indirmenin çubuğu dosyanın tamamıdır; her parça kendi başlangıcından dolar.
  Dosya tamamlanınca parçalar birleşip tek bir çubuk olur.
- **Ayağınıza dolanmaz.** Açık ve koyu tema sisteminizi izler. İlerleme görev çubuğundaki düğmede ya da
  Dock simgesinde de görünür.

## İndir

| Sistem | Dosya |
| --- | --- |
| Windows 10 ve 11 | [indir-windows-setup.exe](https://github.com/akancodes/indir/releases/latest/download/indir-windows-setup.exe); kurmadan çalıştırmak için [indir-windows-portable.exe](https://github.com/akancodes/indir/releases/latest/download/indir-windows-portable.exe) |
| macOS 11 ve sonrası, Apple Silicon ve Intel | [indir-mac.dmg](https://github.com/akancodes/indir/releases/latest/download/indir-mac.dmg) |
| Linux | [AppImage](https://github.com/akancodes/indir/releases/latest/download/indir-linux-amd64.AppImage) · Debian, Ubuntu ve Mint için [.deb](https://github.com/akancodes/indir/releases/latest/download/indir-linux-amd64.deb) · Fedora ve openSUSE için [.rpm](https://github.com/akancodes/indir/releases/latest/download/indir-linux-x86_64.rpm) |

### İlk açılışta

indir ücretli bir Apple ya da Microsoft sertifikasıyla imzalı değil, bu yüzden sisteminiz ilk çalıştırmada bir
kez sorar:

- **Windows:** SmartScreen *Windows bilgisayarınızı korudu* derse **Ek bilgi**'ye, sonra **Yine de çalıştır**'a
  tıklayın. Kurulum yalnızca sizin kullanıcınız için yapılır, yönetici izni gerekmez.
- **macOS:** indir'i bir kez açın. macOS uygulamayı denetleyemediğini söyleyince **Sistem Ayarları →
  Gizlilik ve Güvenlik**'e gidin, aşağı kaydırıp **Yine de Aç**'a tıklayın.
- **Linux:** AppImage dosyasını çalıştırılabilir yapın (`chmod +x indir-linux-amd64.AppImage`) ve çalıştırın;
  ya da .deb veya .rpm paketini kurun.

## Kullanımı

1. Bir dosyanın linkini kopyalayın. Tarayıcıda indirme linkine sağ tıklayıp *Bağlantıyı kopyala*'yı seçin.
2. indir'e yapıştırıp Enter'a basın. Her satıra bir link olmak üzere birkaç linki birden yapıştırabilir ya da
   bir linki pencereye sürükleyebilirsiniz.
3. Bu kadar. Listeden duraklatabilir, devam ettirebilir, dosyayı açabilir ya da klasöründe gösterebilirsiniz;
   gerisi için indirmeye sağ tıklayın.

**Ayarlar**'da (sağ altta) indirmelerin nereye kaydedileceği (başta İndirilenler klasörünüz), her indirmenin kaç
bağlantı açabileceği (1, 2, 4, 8 ya da 16), aynı anda kaç indirmenin süreceği (başta 3) ve bir hız sınırı (başta
kapalı) var. **When done** (altta, bir şey inerken) hiçbir şey kalmayınca bilgisayarı uyutur, kapatır ya da
indir'den çıkar. **Kısayollar:** Ctrl+L (Mac'te ⌘L) link alanına götürür, Ctrl+, (⌘,) Ayarlar'ı açar.

## Nasıl çalışır

1. **Önce küçük bir soru.** indir sunucudan dosyanın yalnızca ilk baytını ister. `206 Partial Content` cevabı,
   sunucunun dosyanın istenen her parçasını gönderebildiğini ve dosyanın ne kadar büyük olduğunu söyler.
   Gönderemeyen bir sunucudan dosya tek, sıradan bir bağlantıyla alınır.
2. **Dosya bölünür.** Her bağlantıya bir tane düşecek, hiçbiri 1 MB'tan küçük olmayacak şekilde eşit parçalara.
   Her bağlantı kendi parçasını bir HTTP `Range` isteğiyle ister; sunucuda değişen bir dosya fark edilsin diye
   isteğe `If-Range` de eklenir. Parça, indirmenin sonunda duracağı yerin yanındaki `.indir` dosyasında doğrudan
   kendi yerine yazılır. Bağlantılar bilerek HTTP/1.1 konuşur: HTTP/2 hepsini tek bir
   TCP bağlantısından taşırdı, indirme yöneticisinin kaçındığı şey de tam olarak bu.
3. **Boşa çıkan bağlantılar diğerlerine yardım eder.** İşini bitiren bir bağlantı, hâlâ inmekte olan en büyük
   parçanın ikinci yarısını devralır. 5 saniyedir hiçbir şey göndermeyen ya da başka bir bağlantının hızının
   on altıda birinde sürünen bir bağlantının parçasının tamamı devralınır ve o bağlantı kapanır; bazı sunucular
   fazladan açılan bağlantılara bilerek damla damla gönderir.
4. **Hız izlenir.** Bağlantılar böyle sürünürse indir birkaç saniyeliğine tek bağlantı dener ve hepsinin
   toplamından iyiyse onda kalır.
5. **Diske ulaşmayan hiçbir şey sayılmaz.** indir iki saniyede bir, yazdıklarını kalıcı olarak diske koyması için
   sisteme seslenir; kaydettiği ilerleme yalnızca o baytları sayar. Çökme ya da elektrik kesintisi en fazla
   birkaç saniyeye mal olur. Her bayt yerine
   gelince `.indir` dosyası asıl adını alır. Var olan bir dosyanın üzerine asla yazılmaz: ikinci bir
   `rapor.pdf`, `rapor (1).pdf` olur.
6. **Hız sınırı için tek bütçe.** Sınır konunca her indirmenin her bağlantısı, aldığını tutmadan önce saniyedeki
   bayt bütçesinden payını alır; bütçe bitmişse bekler. Bekleyen bağlantı hiçbir şey okumaz, böylece sunucu da
   ona göre yavaşlar; sırasını bekleyen bir bağlantı da asla takılmış sanılmaz.

İndirme motoru [`engine/`](engine) klasöründe, kendi başına bir Rust kütüphanesi. Testleri; yavaşlayabilen,
bağlantı koparan, fazla bağlantıyı reddeden, damla damla gönderen, susan ya da dosyayı yolda değiştiren yerel bir
sunucudan indirme yapar. Terminalden de çalışır:

```bash
cargo run --release -p indir-engine --example get -- https://example.com/buyuk.iso
```

## Bilmekte fayda var

- **Oturum açmanızı isteyen linkler çalışmaz.** Bazı bulut depolama, ders ve ücretli indirme siteleri dosyayı
  yalnızca çerezlerinizi bilen tarayıcınıza verir; indir'de o çerezler yok. Doğrudan linkler çalışır: sürüm
  sayfaları, yansı sunucular, CDN'ler ve herkese açık dosya linkleri.
- **Gizlilik.** indir, sizin başlattığınız indirmeler dışında hiçbir yere bir şey göndermez; açılırken GitHub'a
  yalnızca yeni bir sürüm çıkıp çıkmadığını sorar. Çıktıysa durum çubuğundaki bir not o sürüme götürür. Panoya
  yalnızca penceresini öne getirdiğinizde bakar ve orada bulduğunu yalnızca baştan sona linkse kullanır.
- **Bir Mac'i kapatmak** System Events üzerinden olur; bu yüzden *Shut down*'ı ilk seçtiğinizde macOS, indir'in
  buna izni olup olmadığını sorar. Hayır dediyseniz **Sistem Ayarları → Gizlilik ve Güvenlik → Otomasyon**'dan
  izin verebilirsiniz.
- **Nerede ne tutulur.** İndirme listesi ve ayarlar, uygulama verisi klasörünüzdeki iki küçük JSON dosyasıdır:
  Windows'ta `%APPDATA%\dev.akan.indir`, macOS'ta `~/Library/Application Support/dev.akan.indir`, Linux'ta
  `~/.local/share/dev.akan.indir`.
- **Adlar temizlenir.** Sunucudan gelen bir addan Windows, macOS ya da Linux'un kaldıramayacağı karakterler ve
  `rapor.exe`'yi PDF gibi gösterebilecek görünmez karakterler çıkarılır.
- **Bir indirmeyi kaldırmak** onu listeden çıkarır. Bitmiş dosya olduğu yerde kalır; bitmemiş birinin `.indir`
  dosyası silinir.

## Kaynaktan derlemek

[Rust](https://rustup.rs), Node.js 18 ya da sonrası, Linux'ta da Tauri'nin
[gereksinimlerindeki](https://v2.tauri.app/start/prerequisites/) sistem kütüphaneleri (WebKitGTK ve diğerleri)
gerekir.

```bash
npm install
npm run dev      # uygulama; ui/ klasörünü değiştirdikçe yenilenir
npm run build    # sisteminiz için bir kurulum dosyası, target/release/bundle içinde
cargo test       # bütün testler; motorunkiler pencere gerektirmez
```

| Klasör | İçinde ne var |
| --- | --- |
| [`engine/`](engine) | İndirme motoru: ilk soru, parçalar, bağlantılar, hız izleme ve kaldığı yerden devam. Arayüzü yok. |
| [`src-tauri/`](src-tauri) | Onu saran, [Tauri](https://tauri.app) ile yapılmış masaüstü uygulaması: liste, ayarlar ve pencerenin çağırdığı komutlar. |
| [`ui/`](ui) | Pencere: düz HTML, CSS ve JavaScript; derleme adımı yok. |

Hata bildirimleri ve pull request'ler memnuniyetle karşılanır. indir küçük kalmak istiyor, bu yüzden yeni bir
özelliğin yerini hak etmesi gerekiyor.

## Lisans

[MIT](LICENSE) © 2026 Bayram Akan
