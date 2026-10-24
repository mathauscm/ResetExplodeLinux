//! Formato de texto VDF/ACF da Valve (libraryfolders.vdf, appmanifest_*.acf, localconfig.vdf...).
//! Portado do Vdf.cs, mas só a leitura: o app nunca grava nos arquivos da Steam.

/// Valor de uma chave: texto ou um bloco `{ ... }`.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Node(Node),
}

/// Bloco do VDF. Mantém a ordem original; as chaves são comparadas sem diferenciar maiúsculas
/// (a Steam grava "Software"/"software" e "Valve"/"valve" conforme a versão).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Node {
    pub items: Vec<(String, Value)>,
}

impl Node {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.items.iter().find_map(|(k, v)| match v {
            Value::Str(s) if k.eq_ignore_ascii_case(key) => Some(s.as_str()),
            _ => None,
        })
    }

    pub fn child(&self, key: &str) -> Option<&Node> {
        self.items.iter().find_map(|(k, v)| match v {
            Value::Node(n) if k.eq_ignore_ascii_case(key) => Some(n),
            _ => None,
        })
    }

    /// Segue um caminho de blocos: `path(&["UserLocalConfigStore", "Software", "Valve"])`.
    pub fn path(&self, keys: &[&str]) -> Option<&Node> {
        keys.iter().try_fold(self, |n, k| n.child(k))
    }

    pub fn children(&self) -> impl Iterator<Item = (&str, &Node)> {
        self.items.iter().filter_map(|(k, v)| match v {
            Value::Node(n) => Some((k.as_str(), n)),
            _ => None,
        })
    }

    pub fn get_u64(&self, key: &str) -> Option<u64> {
        self.get(key)?.trim().parse().ok()
    }
}

/// Lê o texto inteiro. Arquivo malformado não dá erro: devolve o que deu para entender até ali.
pub fn parse(text: &str) -> Node {
    let mut lx = Lexer { s: text.as_bytes(), i: 0 };
    block(&mut lx)
}

enum Tok {
    Str(String),
    Open,
    Close,
}

struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
}

impl Lexer<'_> {
    fn skip_space(&mut self) {
        while self.i < self.s.len() {
            let c = self.s[self.i];
            if c.is_ascii_whitespace() {
                self.i += 1;
            } else if c == b'/' && self.s.get(self.i + 1) == Some(&b'/') {
                while self.i < self.s.len() && self.s[self.i] != b'\n' {
                    self.i += 1;
                }
            } else {
                break;
            }
        }
    }

    fn next(&mut self) -> Option<Tok> {
        loop {
            self.skip_space();
            let c = *self.s.get(self.i)?;
            match c {
                b'{' => {
                    self.i += 1;
                    return Some(Tok::Open);
                }
                b'}' => {
                    self.i += 1;
                    return Some(Tok::Close);
                }
                b'"' => {
                    self.i += 1;
                    let mut out = Vec::new();
                    while let Some(&c) = self.s.get(self.i) {
                        self.i += 1;
                        match c {
                            b'"' => break,
                            b'\\' => {
                                let e = self.s.get(self.i).copied().unwrap_or(b'\\');
                                self.i += 1;
                                out.push(match e {
                                    b'n' => b'\n',
                                    b't' => b'\t',
                                    b'r' => b'\r',
                                    other => other,
                                });
                            }
                            _ => out.push(c),
                        }
                    }
                    return Some(Tok::Str(String::from_utf8_lossy(&out).into_owned()));
                }
                // condicionais como [$WIN32] depois de um valor: ignorados
                b'[' => {
                    while self.i < self.s.len() && self.s[self.i] != b']' {
                        self.i += 1;
                    }
                    self.i += 1;
                }
                _ => {
                    let start = self.i;
                    while self.i < self.s.len() && !self.s[self.i].is_ascii_whitespace() && !b"{}\"".contains(&self.s[self.i]) {
                        self.i += 1;
                    }
                    return Some(Tok::Str(String::from_utf8_lossy(&self.s[start..self.i]).into_owned()));
                }
            }
        }
    }
}

fn block(lx: &mut Lexer) -> Node {
    let mut node = Node::default();
    loop {
        let key = match lx.next() {
            Some(Tok::Str(k)) => k,
            Some(Tok::Open) => {
                // bloco sem chave: lê e descarta para não perder o alinhamento
                block(lx);
                continue;
            }
            Some(Tok::Close) | None => return node,
        };
        match lx.next() {
            Some(Tok::Open) => node.items.push((key, Value::Node(block(lx)))),
            Some(Tok::Str(v)) => node.items.push((key, Value::Str(v))),
            Some(Tok::Close) | None => {
                node.items.push((key, Value::Str(String::new())));
                return node;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn libraryfolders() {
        let t = r#""libraryfolders"
{
	"0"
	{
		"path"		"/home/u/.local/share/Steam"
		"apps"
		{
			"228980"		"242214525"
		}
	}
	"1"
	{
		"path"		"/mnt/jogos/SteamLibrary"
		"apps" { "1174180" "128281316135" }
	}
}"#;
        let root = parse(t);
        let lf = root.child("LibraryFolders").unwrap();
        let paths: Vec<&str> = lf.children().filter_map(|(_, n)| n.get("path")).collect();
        assert_eq!(paths, ["/home/u/.local/share/Steam", "/mnt/jogos/SteamLibrary"]);
        assert_eq!(lf.child("1").unwrap().child("apps").unwrap().get_u64("1174180"), Some(128281316135));
    }

    #[test]
    fn escapes_comments_and_conditionals() {
        let t = "// comentário\n\"a\" { \"p\" \"C:\\\\Program Files\\\\Steam\" \"q\" \"diz \\\"oi\\\"\" [$WIN32] solto valor }";
        let a = parse(t);
        let a = a.child("a").unwrap();
        assert_eq!(a.get("p"), Some(r"C:\Program Files\Steam"));
        assert_eq!(a.get("q"), Some("diz \"oi\""));
        assert_eq!(a.get("solto"), Some("valor"));
    }

    #[test]
    fn path_ignores_case_and_survives_truncation() {
        let t = "\"UserLocalConfigStore\" { \"Software\" { \"valve\" { \"Steam\" { \"apps\" { \"730\" { \"LaunchOptions\" \"-novid\" } } } } }";
        let r = parse(t);
        let apps = r.path(&["UserLocalConfigStore", "Software", "Valve", "Steam", "apps"]).unwrap();
        assert_eq!(apps.child("730").unwrap().get("LaunchOptions"), Some("-novid"));
        // arquivo cortado no meio: devolve o que leu
        let r = parse("\"x\" { \"a\" \"1\" \"b\"");
        assert_eq!(r.child("x").unwrap().get("a"), Some("1"));
    }
}
