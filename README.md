# robopaste

## ❤️ Ủng hộ / Support

Nếu dự án này hữu ích với bạn, hãy ủng hộ mình một ly cà phê nhé. Cảm ơn rất nhiều!

If this project helps you, consider buying me a coffee. Thank you!

<p align="center">
  <img src="assets/support/donate-qr.jpg" alt="Ủng hộ / Support - LÂM HIỀN TÂM (MoMo · VietQR · napas 247)" width="360">
</p>

<p align="center"><b>LÂM HIỀN TÂM</b> - MoMo · VietQR · napas 247</p>

## 🤖 How this was built

This project, a Rust tray app with a low-level keyboard hook, a filesystem safety gate and an MSI installer, was built by an autonomous loop skill (brainstorm → plan → cook → review), powered by **[AgentKit Engineer](https://agentkit.best/engineer?ref=EPU2ZB5U)**.

Want the same workflow? [**Get AgentKit Engineer with 50% off**](https://agentkit.best/engineer?ref=EPU2ZB5U).

---

**Ctrl+C / Ctrl+X then Ctrl+V a folder in Windows Explorer, and it is copied or moved by `robocopy` instead of Explorer's own copy engine.** A small Rust tray app for Windows 11: no DLL injected into Explorer, no file associations touched, exit the app and stock paste is back.

## Why

Explorer's folder copy is slow on trees with many small files, stops and asks on every conflict, and gives up when a network share hiccups. `robocopy` is multithreaded, merges without prompting and skips what fails, but you have to type both paths by hand. robopaste puts it behind the keys you already use.

> Status: the safety gate, robocopy runner and Explorer tab resolver are covered by automated tests. The keyboard hook and tray have not been through a full manual test pass yet. Try it on throwaway folders first.

---

Ứng dụng khay hệ thống (tray) cho Windows 11, viết bằng Rust. Khi bạn bấm Ctrl+V trong danh sách tệp của Explorer (hoặc trên Desktop) và clipboard chỉ chứa thư mục, robopaste chặn phím và chạy `robocopy` trong một cửa sổ console riêng thay cho bản dán mặc định của Explorer:

```
robocopy "<nguồn>" "<đích>\<tên thư mục>" /E /R:0 /W:0 /IT /MT:16              (sao chép)
robocopy "<nguồn>" "<đích>\<tên thư mục>" /E /R:0 /W:0 /IT /MT:16 /MOVE /XJ    (cắt)
```

Nhiều thư mục được chạy lần lượt trong cùng một console. Console tự đóng khi mọi mã thoát nhỏ hơn 8; nếu có lỗi (mã thoát từ 8 trở lên) console giữ nguyên kèm bản tóm tắt.

## Build

Cần Rust (đã thử với 1.95).

```
cargo build --release
cargo test
```

Tệp chạy nằm ở `target\release\robopaste.exe`. Có thể sao chép tệp này đi bất kỳ đâu.

## Cài bằng MSI

Cần WiX v4 (`dotnet tool install --global wix`).

```
powershell -ExecutionPolicy Bypass -File installer\build-msi.ps1
msiexec /i installer\out\robopaste-0.1.0.msi
```

MSI cài theo người dùng hiện tại, không cần quyền admin: tệp chạy vào `%LocalAppData%\Programs\robopaste\` và có lối tắt `robopaste` trong Start Menu. Trình cài không tự khởi động ứng dụng và không bật tự chạy cùng Windows.

## Chạy và tự khởi động

Chạy `robopaste.exe`: biểu tượng xuất hiện ở khay hệ thống. Windows 11 mặc định có thể giấu biểu tượng mới trong khay mở rộng (mũi tên `^`), hãy kéo ra ngoài nếu muốn thấy luôn. Chỉ một bản chạy cùng lúc; lần chạy thứ hai tự thoát im lặng.

Menu khay (bấm chuột trái hoặc phải):

- **Enabled**: bật/tắt việc chặn Ctrl+V. Tắt thì Explorer dán như bình thường. Bật lại cũng cài lại hook nếu Windows đã tự gỡ.
- **Start with Windows**: ghi hoặc xóa giá trị `robopaste` trong `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (chỉ tài khoản hiện tại, không cần quyền admin).
- **Exit**: gỡ biểu tượng và hook, thoát ngay; Explorer trở lại hành vi gốc.

Chế độ phụ: `robopaste.exe --where` (đợi 3 giây rồi in thư mục của cửa sổ Explorer đang ở trên cùng, in ra console đã mở nó), và `robopaste.exe --run [--move] --dest <thư mục> <nguồn>...` (chạy robocopy trong console riêng, dùng nội bộ).

## Khi nào Explorer vẫn tự xử lý việc dán

robopaste trả phím lại cho Explorer khi:

- đang đổi tên (F2), gõ vào thanh địa chỉ, ô tìm kiếm hay ngăn điều hướng bên trái;
- clipboard có bất kỳ tệp nào (không chỉ thư mục);
- đích là thư mục ảo (This PC, Library, tệp zip, kết quả tìm kiếm);
- dán vào chính thư mục cha của nguồn (Explorer tạo "tên - Copy");
- đích là chính nguồn hoặc nằm bên trong nguồn, kể cả khi đi qua junction, symlink, ổ `subst` hay ổ mạng (so sánh bằng danh tính tệp, không chỉ chuỗi đường dẫn);
- nguồn là junction hoặc symlink, hoặc `đích\tên` đã tồn tại và là liên kết;
- không kiểm tra được an toàn (lỗi bất kỳ thì nhường cho Explorer);
- các cửa sổ khác (Notepad, trình duyệt, terminal) và tổ hợp Ctrl+Shift+V.

Trong thẻ Explorer nhiều tab, thư mục đích là thư mục của tab đang mở.

## Giới hạn đã biết

- Không chặn mục Paste trong menu chuột phải hay kéo thả.
- Explorer chạy bằng quyền admin thì hook (không admin) không thấy phím, bản dán mặc định diễn ra.
- Không có hoàn tác. Cắt trong cùng một ổ là sao chép rồi xóa, chậm hơn đổi tên của Explorer.
- Khi cắt, junction và symlink (kể cả symlink tệp) bên trong thư mục được bỏ qua (`/XJ`): chúng ở lại nguồn cùng các thư mục chứa chúng, nên thư mục nguồn không bị xóa hẳn (mã thoát 1, console tự đóng). Khi sao chép thì junction được đi theo và nội dung được nhân đôi; junction trỏ ngược lên thư mục cha sẽ lặp tới giới hạn độ dài đường dẫn (mã thoát 9, console giữ lại).
- Cắt vào cây thư mục giống hệt đã có ở đích thì nguồn được giữ nguyên.
- Gộp thư mục có thể ghi đè tệp đích mới hơn bằng tệp nguồn cũ hơn.
- Cùng một thư mục máy chủ được chia sẻ dưới hai tên share khác nhau thì không nhận ra là một. Một junction nằm sâu trong thư mục đích đã có sẵn mà trỏ ngược vào nguồn cũng không bị phát hiện.
- Mục đã cắt vẫn mờ trong Explorer đến khi làm mới.
- Exe chưa ký nhưng cài hook bàn phím toàn cục nên một số phần mềm diệt virus có thể cảnh báo. robopaste chỉ xem phím V và trạng thái Ctrl/Shift/Alt/Win, không ghi lại phím nào.

## Gỡ cài đặt

Chọn **Exit** từ menu khay, bỏ chọn **Start with Windows** trước đó (hoặc xóa giá trị `robopaste` trong khóa Run), rồi xóa tệp `robopaste.exe`. Nếu đã cài bằng MSI thì gỡ trong Settings → Apps → Installed apps (mục `robopaste`) sau khi đã **Exit**.
