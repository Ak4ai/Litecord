# Litecord 🚀

Um cliente leve e otimizado do Discord construído em **Rust** utilizando **Slint** para a interface gráfica, **Tokio** para async/networking e **CPAL** / **Opus** para áudio.

---

## 🛠️ Requisitos e Compilação

- **Rust** (versão 2021 edition ou mais recente)
- **Cargo**

### 📦 Rodar a aplicação:

```bash
cargo run
```

### ⚡ Observação sobre OneDrive e Target Directory:
Para evitar que o OneDrive tente sincronizar gigabytes de arquivos temporários de compilação da pasta `target/`, este projeto possui uma configuração em `.cargo/config.toml` redirecionando o `target-dir`.

Se você clonar o repositório em outra máquina (ex: notebook) e o usuário for diferente, você pode ajustar o caminho em `.cargo/config.toml` ou comentar a linha `target-dir` para usar a pasta `target/` padrão.

---

## 📂 Estrutura do Projeto

- `src/main.rs`: Ponto de entrada da aplicação, gerenciamento de estado da UI e canais.
- `src/gateway.rs`: Conexão WebSocket com a Gateway do Discord e eventos em tempo real.
- `src/http.rs`: Requisições HTTP REST para a API do Discord.
- `src/tray.rs`: Integração com a bandeja do sistema (System Tray).
- `ui/appwindow.slint`: Interface do usuário desenvolvida em Slint UI.
- `.cargo/config.toml`: Configurações do Cargo.
