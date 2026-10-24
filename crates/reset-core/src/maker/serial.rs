//! Monitor serial: abre a porta em modo cru (8N1, sem controle de fluxo) com termios, lê e escreve.
//! A leitura usa `poll` com tempo limite, então uma thread pode ler enquanto outra escreve no mesmo descritor.

use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::Duration;

/// Velocidades oferecidas na interface (as mais usadas com Arduino, ESP e GPS).
pub const BAUDS: &[u32] = &[300, 1200, 2400, 4800, 9600, 19200, 38400, 57600, 74880, 115200, 230400, 250000, 460800, 500000, 921600, 1000000, 2000000];

pub struct SerialPort {
    fd: OwnedFd,
}

fn last_err(what: &str) -> String {
    let e = std::io::Error::last_os_error();
    let hint = match e.raw_os_error() {
        Some(libc::EACCES) | Some(libc::EPERM) => " (sem permissão: entre no grupo das portas seriais, na tela Portas seriais)",
        Some(libc::EBUSY) => " (outro programa está com a porta presa)",
        Some(libc::ENOENT) | Some(libc::ENODEV) | Some(libc::ENXIO) => " (a placa foi desconectada?)",
        Some(libc::EIO) => " (a placa foi desconectada ou parou de responder)",
        _ => "",
    };
    format!("{what}: {e}{hint}")
}

/// Constante termios da velocidade. As que o Linux não tem como constante (74880, 250000) usam BOTHER.
fn speed_const(baud: u32) -> Option<libc::speed_t> {
    Some(match baud {
        300 => libc::B300,
        1200 => libc::B1200,
        2400 => libc::B2400,
        4800 => libc::B4800,
        9600 => libc::B9600,
        19200 => libc::B19200,
        38400 => libc::B38400,
        57600 => libc::B57600,
        115200 => libc::B115200,
        230400 => libc::B230400,
        460800 => libc::B460800,
        500000 => libc::B500000,
        921600 => libc::B921600,
        1000000 => libc::B1000000,
        2000000 => libc::B2000000,
        _ => return None,
    })
}

impl SerialPort {
    pub fn open(path: &str, baud: u32) -> Result<SerialPort, String> {
        if !BAUDS.contains(&baud) {
            return Err(format!("velocidade não suportada: {baud}"));
        }
        let c = CString::new(path).map_err(|_| "caminho inválido".to_string())?;
        let raw = unsafe { libc::open(c.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC) };
        if raw < 0 {
            return Err(last_err(&format!("não deu para abrir {path}")));
        }
        let port = SerialPort { fd: unsafe { OwnedFd::from_raw_fd(raw) } };
        port.configure(baud)?;
        Ok(port)
    }

    fn configure(&self, baud: u32) -> Result<(), String> {
        let fd = self.fd.as_raw_fd();
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut t) != 0 {
                return Err(last_err("a porta não aceitou configuração (não é uma porta serial?)"));
            }
            libc::cfmakeraw(&mut t);
            t.c_cflag |= libc::CLOCAL | libc::CREAD;
            t.c_cflag &= !(libc::CSTOPB | libc::PARENB | libc::CRTSCTS);
            t.c_iflag &= !(libc::IXON | libc::IXOFF | libc::IXANY);
            t.c_cc[libc::VMIN] = 0;
            t.c_cc[libc::VTIME] = 0;
            match speed_const(baud) {
                Some(s) => {
                    libc::cfsetispeed(&mut t, s);
                    libc::cfsetospeed(&mut t, s);
                    if libc::tcsetattr(fd, libc::TCSANOW, &t) != 0 {
                        return Err(last_err("não deu para configurar a porta"));
                    }
                }
                None => {
                    if libc::tcsetattr(fd, libc::TCSANOW, &t) != 0 {
                        return Err(last_err("não deu para configurar a porta"));
                    }
                    set_custom_speed(fd, baud)?;
                }
            }
            libc::tcflush(fd, libc::TCIOFLUSH);
        }
        Ok(())
    }

    /// Espera até `timeout` por dados. Ok(0) = nada chegou; Err = a porta caiu (placa desconectada).
    pub fn read_timeout(&self, buf: &mut [u8], timeout: Duration) -> Result<usize, String> {
        let fd = self.fd.as_raw_fd();
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        let r = unsafe { libc::poll(&mut pfd, 1, timeout.as_millis() as i32) };
        if r < 0 {
            let e = std::io::Error::last_os_error();
            return if e.kind() == std::io::ErrorKind::Interrupted { Ok(0) } else { Err(last_err("erro ao esperar dados")) };
        }
        if r == 0 {
            return Ok(0);
        }
        if pfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 && pfd.revents & libc::POLLIN == 0 {
            return Err("a placa foi desconectada".into());
        }
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            return match e.raw_os_error() {
                Some(libc::EAGAIN) | Some(libc::EINTR) => Ok(0),
                _ => Err(last_err("erro de leitura")),
            };
        }
        if n == 0 {
            // pronto para ler mas sem bytes: em porta USB-serial isso é desconexão
            return Err("a placa foi desconectada".into());
        }
        Ok(n as usize)
    }

    pub fn write_all(&self, mut data: &[u8]) -> Result<(), String> {
        let fd = self.fd.as_raw_fd();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !data.is_empty() {
            let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
            if n > 0 {
                data = &data[n as usize..];
                continue;
            }
            let e = std::io::Error::last_os_error();
            if !matches!(e.raw_os_error(), Some(libc::EAGAIN) | Some(libc::EINTR)) {
                return Err(last_err("erro ao enviar"));
            }
            if std::time::Instant::now() > deadline {
                return Err("a placa não está recebendo (tempo esgotado ao enviar)".into());
            }
            let mut pfd = libc::pollfd { fd, events: libc::POLLOUT, revents: 0 };
            unsafe { libc::poll(&mut pfd, 1, 200) };
        }
        Ok(())
    }

    fn set_lines(&self, dtr: bool, rts: bool) -> Result<(), String> {
        let fd = self.fd.as_raw_fd();
        let mut bits: libc::c_int = 0;
        unsafe {
            if libc::ioctl(fd, libc::TIOCMGET, &mut bits) != 0 {
                return Err(last_err("a porta não aceita DTR/RTS"));
            }
            bits = if dtr { bits | libc::TIOCM_DTR } else { bits & !libc::TIOCM_DTR };
            bits = if rts { bits | libc::TIOCM_RTS } else { bits & !libc::TIOCM_RTS };
            if libc::ioctl(fd, libc::TIOCMSET, &bits) != 0 {
                return Err(last_err("a porta não aceita DTR/RTS"));
            }
        }
        Ok(())
    }

    /// Reinicia a placa pelas linhas DTR/RTS, como as IDEs fazem.
    /// ESP32/ESP8266 (circuito de dois transistores): RTS segura o EN em 0; soltando com DTR também solto,
    /// o GPIO0 fica em 1 e ela volta no modo normal (não no de gravação).
    /// Arduino Uno/Nano (capacitor no DTR): a volta do DTR para ativo dá o pulso de reset.
    pub fn pulse_reset(&self) -> Result<(), String> {
        self.set_lines(false, true)?;
        std::thread::sleep(Duration::from_millis(120));
        self.set_lines(false, false)?;
        std::thread::sleep(Duration::from_millis(60));
        self.set_lines(true, true)
    }
}

/// Velocidade fora da tabela (74880 do boot do ESP8266, 250000 de impressoras 3D): termios2 + BOTHER.
fn set_custom_speed(fd: i32, baud: u32) -> Result<(), String> {
    unsafe {
        let mut t: libc::termios2 = std::mem::zeroed();
        if libc::ioctl(fd, libc::TCGETS2, &mut t) != 0 {
            return Err(last_err("velocidade não suportada por esta porta"));
        }
        t.c_cflag &= !libc::CBAUD;
        t.c_cflag |= libc::BOTHER;
        t.c_ispeed = baud;
        t.c_ospeed = baud;
        if libc::ioctl(fd, libc::TCSETS2, &t) != 0 {
            return Err(last_err("velocidade não suportada por esta porta"));
        }
    }
    Ok(())
}

/// Quantos bytes do fim de `buf` são o começo de um caractere UTF-8 ainda incompleto (até 3).
/// O monitor guarda esses bytes para juntar com o próximo pedaço, senão acentos quebram ao meio.
pub fn utf8_tail(buf: &[u8]) -> usize {
    for back in 1..=3.min(buf.len()) {
        let b = buf[buf.len() - back];
        if b & 0xC0 == 0x80 {
            continue; // byte de continuação: olha o anterior
        }
        let need = if b & 0xE0 == 0xC0 {
            2
        } else if b & 0xF0 == 0xE0 {
            3
        } else if b & 0xF8 == 0xF0 {
            4
        } else {
            1
        };
        return if need > back { back } else { 0 };
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_pela_metade() {
        let s = "temperatura: 25 °C".as_bytes();
        assert_eq!(utf8_tail(s), 0);
        let cut = &s[..s.len() - 2]; // corta no meio do "°"
        assert_eq!(utf8_tail(cut), 1);
        assert_eq!(utf8_tail("ação".as_bytes()), 0);
        assert_eq!(utf8_tail(&[b'a', 0xE2, 0x82]), 2); // "€" sem o último byte
        assert_eq!(utf8_tail(&[0xF0, 0x9F, 0x98]), 3);
        assert_eq!(utf8_tail(&[]), 0);
    }

    /// Um pseudoterminal faz o papel da placa: o lado "mestre" é o Arduino, o "escravo" é a porta.
    #[test]
    fn conversa_com_pty() {
        unsafe {
            let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            assert!(master >= 0);
            assert_eq!(libc::grantpt(master), 0);
            assert_eq!(libc::unlockpt(master), 0);
            let mut name = [0 as libc::c_char; 64];
            assert_eq!(libc::ptsname_r(master, name.as_mut_ptr(), name.len()), 0);
            let path = std::ffi::CStr::from_ptr(name.as_ptr()).to_string_lossy().into_owned();
            for baud in [115200, 74880] {
                let port = SerialPort::open(&path, baud).unwrap();
                let msg = b"ola, placa\n";
                assert_eq!(libc::write(master, msg.as_ptr().cast(), msg.len()), msg.len() as isize);
                let mut buf = [0u8; 64];
                let n = port.read_timeout(&mut buf, Duration::from_millis(500)).unwrap();
                assert_eq!(&buf[..n], msg);
                assert_eq!(port.read_timeout(&mut buf, Duration::from_millis(20)).unwrap(), 0);
                port.write_all(b"LED=1\r\n").unwrap();
                let mut back = [0u8; 16];
                let mut pfd = libc::pollfd { fd: master, events: libc::POLLIN, revents: 0 };
                libc::poll(&mut pfd, 1, 500);
                let n = libc::read(master, back.as_mut_ptr().cast(), back.len());
                assert_eq!(&back[..n as usize], b"LED=1\r\n");
            }
            libc::close(master);
        }
    }

    #[test]
    fn velocidades() {
        assert!(speed_const(115200).is_some());
        assert!(speed_const(74880).is_none()); // vai por BOTHER
        assert!(SerialPort::open("/dev/null", 12345).is_err());
        // /dev/null abre, mas não é terminal: recusa com mensagem
        assert!(SerialPort::open("/dev/null", 9600).is_err());
    }
}
