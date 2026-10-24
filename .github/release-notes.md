<!-- reset-explode:instalar -->
## Qual arquivo baixar

Escolha pela sua distribuição. Todos são para PC de 64 bits (x86_64) e trazem o app completo.

| Sua distribuição | Arquivo | Como instalar |
|---|---|---|
| **Ubuntu** 22.04 ou mais novo, **Linux Mint** 21+, **Pop!_OS** 22.04+, **Zorin OS** 17+, **elementary OS** 7+ | `reset-explode-{{VERSION}}-ubuntu-debian-amd64.deb` | dois cliques no arquivo, ou `sudo apt install ./reset-explode-{{VERSION}}-ubuntu-debian-amd64.deb` |
| **Debian** 12 ou mais novo | `reset-explode-{{VERSION}}-ubuntu-debian-amd64.deb` | `sudo apt install ./reset-explode-{{VERSION}}-ubuntu-debian-amd64.deb` |
| **Fedora** 38 ou mais novo | `reset-explode-{{VERSION}}-fedora-x86_64.rpm` | dois cliques no arquivo, ou `sudo dnf install ./reset-explode-{{VERSION}}-fedora-x86_64.rpm` |
| **Arch**, **CachyOS**, **Manjaro**, **EndeavourOS**, **openSUSE** e qualquer outra | `reset-explode-{{VERSION}}-qualquer-distro-x86_64.AppImage` | botão direito → Propriedades → marque **Executável** e abra (ou `chmod +x` no arquivo e rode) |

- O `.deb` e o `.rpm` instalam as dependências sozinhos e criam o atalho **RESET EXPLODE** no menu de aplicativos.
- O AppImage roda direto, sem instalar e sem precisar de `fuse2`. Também serve no Ubuntu, no Debian e no Fedora, para quem preferir não instalar.
- `SHA256SUMS` serve para conferir o download: `sha256sum -c SHA256SUMS --ignore-missing`.

**Opcionais** (o app avisa quando faltam): `openrgb` para a iluminação RGB, `mangohud` para o overlay, driver NVIDIA para a ficha e o overclock da GPU. Ações que mudam o sistema pedem a senha de administrador pela janela do próprio sistema.

Port para Linux do [RESET EXPLODE](https://github.com/GustavoReset/ResetExplode) de Gustavo (GustavoReset), licença MIT.
