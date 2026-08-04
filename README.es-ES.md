

<div align="center">

<img src="logo-readme.png" alt="OxiBrowser logo" width="120">

# 🌐 OxiBrowser

**El navegador headless construido en Rust puro para agentes de IA.**

No es un fork de Chromium. No es un envoltorio de C++. Un motor de navegador escrito desde cero en Rust,
diseñado desde el primer día para automatización, extracción de datos web y flujos de trabajo impulsados por IA.

[![CI](https://img.shields.io/github/actions/workflow/status/a7garden/oxibrowser/ci.yml?branch=main&style=flat-square&logo=github&label=CI)](https://github.com/a7garden/oxibrowser/actions)
[![Crates.io](https://img.shields.io/crates/v/oxibrowser?style=flat-square&logo=rust&label=crates.io)](https://crates.io/crates/oxibrowser)
[![docs.rs](https://img.shields.io/docsrs/oxibrowser?style=flat-square&label=docs.rs)](https://docs.rs/oxibrowser)
[![GitHub release](https://img.shields.io/github/v/release/a7garden/oxibrowser?style=flat-square&include_prereleases&label=release)](https://github.com/a7garden/oxibrowser/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](https://github.com/a7garden/oxibrowser/blob/main/LICENSE)
[![GitHub stars](https://img.shields.io/github/stars/a7garden/oxibrowser?style=flat-square&logo=github)](https://github.com/a7garden/oxibrowser/stargazers)
[![Rust](https://img.shields.io/badge/rust-1.96%2B-orange?style=flat-square&logo=rust)](https://www.rust-lang.org/)

[Reportar error](https://github.com/a7garden/oxibrowser/issues) · [Solicitar funcionalidad](https://github.com/a7garden/oxibrowser/issues) · [Leer la documentación](https://github.com/a7garden/oxibrowser/blob/main/docs/ARCHITECTURE.md) · [Discord](https://discord.gg/oxibrowser)

</div>

---

<div align="center">

<table>
<tr>
<td align="center"><strong>24 MB</strong><br><sub>Binario estático único</sub></td>
<td align="center"><strong>~50 ms</strong><br><sub>Tiempo de inicio en frío</sub></td>
<td align="center"><strong>~8 MB</strong><br><sub>Memoria base</sub></td>
<td align="center"><strong>554 tests</strong><br><sub>Cobertura completa</sub></td>
<td align="center"><strong>Prioridad a Rust</strong><br><sub>Cadena de herramientas C solo para TLS</sub></td>
</tr>
</table>

<table>
<tr>
<th>OxiBrowser</th>
<th>Headless Chrome</th>
</tr>
<tr>
<td align="center">Binario de 24 MB</td>
<td align="center">Instalación de ~400 MB</td>

</tr>
<tr>
<td align="center">Memoria RAM base de ~8 MB</td>
<td align="center">Memoria RAM base de ~200 MB</td>

</tr>
<tr>
<td align="center">Inicio de ~50 ms</td>
<td align="center">Inicio de ~800 ms</td>

</tr>
<tr>
<td align="center">Rust puro (boa)</td>
<td align="center">C++ (V8)</td>

</tr>
<tr>
<td align="center">MIT</td>
<td align="center">BSD / ToS</td>

</tr>
</table>

</div>

---

## ✨ ¿Por qué OxiBrowser?

**Estás creando agentes de IA que necesitan navegar por la web.** No necesitas un navegador completo con renderizado GPU, salida de audio y soporte para extensiones. Necesitas algo rápido, pequeño y programable.

OxiBrowser está construido exactamente para ese caso de uso:

- 🤖 **Enfoque en agentes de IA** — CLI diseñada para agentes: salida en `--json`, `describe` para esquemas, `skill` para indicaciones (prompts), `session` para pasos múltiples
- ⚡ **Extremadamente rápido** — Inicios en frío de ~50 ms, sin sobrecarga de Chromium, no se requiere Node.js
- 🦀 **Prioridad a Rust** — `boa_engine` (JS, sin V8), `html5ever` (HTML) son Rust puro. TLS usa `btls` (vinculación C de BoringSSL) para emulación sigilosa de huellas. Binario estático único.
- 🔌 **Compatible con CDP** — Puppeteer, Playwright y cualquier cliente del Chrome DevTools Protocol funcionan directamente
- 🛡️ **Seguro por defecto** — Protección contra SSRF con bloqueo CIDR, respeto a `robots.txt`, sin superficie de escape de sandbox
- 📦 **Huella reducida** — Binario de 24 MB, ~8 MB de memoria base. Ejecuta 100 instancias sin esfuerzo

---


## 🆕 ¿Qué hay de nuevo en 0.17.0?

Esta versión cierra la mayor brecha entre un "obtenedor de HTML" y un "navegador headless real": los eventos ahora se comportan como en un navegador, `innerHTML` funciona para inyección de DOM estilo SPA, y `fetch` devuelve un `Response` conforme a la especificación con `arrayBuffer()`.

**Eventos y DOM**
- Los constructores de eventos respetan el diccionario de inicialización: `new MouseEvent('click', { clientX, clientY, ctrlKey, ... })` ahora transporta realmente esos campos. Cubre `MouseEvent`, `KeyboardEvent`, `FocusEvent`, `Event` y un nuevo `DragEvent`.
- `dispatchEvent` establece `event.target` / `event.currentTarget` y devuelve `!defaultPrevented`. `preventDefault` / `stopPropagation` / `stopImmediatePropagation` funcionan en todos los eventos.
- **Burbujeo de eventos** recorre la cadena de padres. Los oyentes se almacenan en un registro de hilo local con clave `nodeId`, por lo que persisten entre reconsultas de objetos de elemento (el error que hacía invisible `parent.addEventListener` para `child.dispatchEvent`).
- `requestAnimationFrame` / `cancelAnimationFrame` ahora se programan correctamente con un límite de 16 ms y pasan un `DOMHighResTimeStamp` a la función de devolución de llamada.
- Accesador `innerText` y global `performance` independiente (`window.performance === performance`).

**HTML y `innerHTML`**
- El asignador `innerHTML` analiza el fragmento mediante `html5ever` e inserta nodos secundarios en la instantánea. El accesador `outerHTML` serializa el nodo de nuevo. Un nuevo módulo `dom_serializer` maneja el viaje de ida y vuelta con un manejo adecuado de elementos vacíos y escape de atributos (12 pruebas unitarias).

**Red**
- `Response.text()` / `json()` / `arrayBuffer()` devuelven Promises con formato de especificación que se resuelven en el cuerpo real de la respuesta. Las opciones de `fetch` `headers` (content-type, accept, authorization, user-agent, cookie) ahora se reenvían.
- El filtro SSRF ahora es consciente del esquema: solo `http`/`https` pasan por las verificaciones de DNS/host. `about:blank` es compatible (la URL de destino predeterminada de Puppeteer/Playwright ahora funciona).

**CDP**
- `Input.dispatchMouseEvent` emite una secuencia real: `mousePressed` → `mousedown`; `mouseReleased` → `mouseup` + `click`; `mouseMoved` → `mousemove`. `Input.dispatchDragEvent` está conectado a un `DragEvent` en el elemento en el punto.

---

## 🚀 Inicio rápido

### Instalación

```bash
cargo install oxibrowser
```

### Obtener una página (legible por humanos)

```bash
$ oxibrowser fetch https://example.com

Example Domain

# Example Domain

This domain is for use in documentation examples...
[Learn more](https://iana.org/domains/example)
```

### Obtener una página (modo agente)

```bash
$ oxibrowser fetch https://example.com --json
{"ok":true,"data":{"url":"https://example.com/","title":"Example Domain","status":200,"markdown":"..."},"meta":{"elapsed_ms":152}}
```

### Extraer datos estructurados

```bash
$ oxibrowser extract https://example.com --links --json
{"ok":true,"data":{"links":["https://iana.org/domains/example"],"title":"Example Domain"}}
```

### Sesión de múltiples pasos (REPL JSON entrada/salida estándar)

```bash
$ oxibrowser session
new
{"ok":true,"data":{"tab_id":"t1"}}
goto t1 https://example.com
{"ok":true,"data":{"status":200,"title":"Example Domain"}}
eval t1 document.title
{"ok":true,"data":{"value":"Example Domain"}}
close t1
{"ok":true,"data":{"closed":"t1"}}
exit
{"ok":true,"data":{"exit":true}}
```

### Iniciar servidor CDP (Puppeteer/Playwright)

```bash
oxibrowser serve --port 9222
```

```javascript
import puppeteer from 'puppeteer-core';

const browser = await puppeteer.connect({
    browserWSEndpoint: 'ws://127.0.0.1:9222',
});

const page = await browser.newPage();
await page.goto('https://news.ycombinator.com');
console.log(await page.title());
await browser.close();
```

---

## 📋 Referencia de la CLI

```
oxibrowser <COMANDO>

COMANDOS:
  fetch      Obtener una URL y devolver contenido (markdown por defecto)
  extract    Extraer datos estructurados (enlaces, texto, elementos)
  run        Ejecutar un script de automatización YAML
  session    REPL JSON interactivo de entrada/salida estándar (22 comandos)
  serve      Iniciar servidor WebSocket CDP
  search     Búsqueda web / GitHub / issues de GitHub (sin necesidad de navegador)
  describe   Imprimir esquema de la CLI como JSON (para agentes)
  skill      Imprimir guía de habilidades para agentes
  version    Imprimir información de versión
```

### fetch — Obtención única de página

```bash
# Legible por humanos (markdown, por defecto)
oxibrowser fetch https://example.com

# Modo agente
oxibrowser fetch https://example.com --json

# Hacer clic y luego leer
oxibrowser fetch https://example.com --click button --wait .result --json

# Resumen rápido de la página
oxibrowser fetch https://example.com --summary --json

# Ejecutar JS
oxibrowser fetch https://example.com --eval "document.title" --json

# Limitar tamaño de respuesta
oxibrowser fetch https://example.com --max-bytes 8000 --json

# Seleccionar campos específicos
oxibrowser fetch https://example.com --fields url,title,status --json
```

### extract — Extracción de datos estructurados

```bash
# Obtener todos los enlaces
oxibrowser extract https://example.com --links --json

# Extraer elementos por selector CSS
oxibrowser extract https://example.com --selector "a" --all --attrs text,href --json

# Título + texto completo
oxibrowser extract https://example.com --title --text --json
```

### session — Automatización de múltiples pasos

```bash
oxibrowser session  # Iniciar REPL

# 22 comandos:
new, goto, back, forward, reload, click, fill, press, type,
select, check, uncheck, scroll, eval, extract, content,
screenshot, wait, close, close --all, list, help, exit
```

### describe — Introspección del agente

```bash
# Compacto (~200 tokens)
oxibrowser describe --compact

# Detalles completos del comando
oxibrowser describe fetch
oxibrowser describe session
```


### search — Búsqueda web / GitHub (sin necesidad de navegador)

```bash
# Búsqueda web (DuckDuckGo)
oxibrowser search "rust async" --engine ddg --max-results 5 --json

# Búsqueda en GitHub
oxibrowser search "memory pool" --source github --json

# Issues de GitHub para un repositorio específico
oxibrowser search "panic on shutdown" --source github-issues --repo a7garden/oxibrowser --json
```
### run — Automatización YAML


```yaml
name: example
steps:
  - step_type: goto
    data:
      goto: https://example.com
  - step_type: content
    data:
      format: markdown
```

```bash
oxibrowser run script.yaml
```

### Formato de salida JSON

Todas las respuestas `--json` siguen el mismo esquema:

```json
{
  "ok": true,
  "data": { ... },
  "meta": { "elapsed_ms": 152 }
}
```

En caso de error:

```json
{
  "ok": false,
  "error": "El esquema de la URL debe ser http o https",
  "error_code": "INVALID_URL"
}
```

**Códigos de salida**: 0=éxito, 1=tiempo de ejecución, 2=validación de entrada, 3=tiempo de espera agotado, 4=red

---

## 🏗 Arquitectura

```
┌──────────────────────────────────────────────────────┐
│            Puppeteer / Playwright / Rust CDP          │
└────────────────────────┬─────────────────────────────┘
                         │ WebSocket CDP
                         ▼
┌──────────────────────────────────────────────────────┐
│               Servidor CDP (10 dominios)              │
│  Browser · DOM · Fetch · Input · Network             │
│  OXI · Page · Runtime · Target                       │
├──────────────────────────────────────────────────────┤
│          Navegador → Sesión → Página → Marco          │
├──────────┬──────────┬──────────────┬─────────────────┤
│  WebAPI  │   Red    │  Motor JS    │ Renderizado CSS │
│  DOM     │  HTTP    │  boa_engine  │  Captura PNG    │
│  Árbol   │ Cookies  │  ES2024+     │  ASCII/Unicode  │
│Almacenam.│  SSRF    │  persistente │  texto→imagen   │
├──────────┴──────────┴──────────────┴─────────────────┤
│   html5ever · encoding_rs · reqwest · image · boa    │
└──────────────────────────────────────────────────────┘
```

### Estructura de Crate

| Crate | Líneas | Propósito |
|-------|--------|-----------|
| [`oxibrowser`](crates/oxibrowser/) | 4,242 | Binario + CLI (8 subcomandos, REPL de sesión, características para agentes) |
| [`oxibrowser-core`](crates/oxibrowser-core/) | 19,794 | Motor del navegador: Sesión, Página, Marco, Motor JS |
| [`oxibrowser-cdp`](crates/oxibrowser-cdp/) | 4,583 | Servidor WebSocket CDP con 10 manejadores de dominios |
| [`oxibrowser-webapi`](crates/oxibrowser-webapi/) | 1,587 | Árbol DOM, selectores CSS, conversión a Markdown |
| **Total** | **30,206** | |

---

## 🌟 Características

### CLI con enfoque en agentes

Diseñada para flujos de trabajo de agentes de IA: sin demonio, sin socket, binario único:

| Característica | Descripción |
|---------|-------------|
| **`--json`** | Salida legible por máquina (opcional, legible por humanos por defecto) |
| **`--max-bytes N`** | Truncar respuesta a N bytes |
| **`--fields a,b,c`** | Seleccionar campos de salida específicos |
| **`--summary`** | Metadatos rápidos de la página (título, enlaces, encabezados) |
| **`describe`** | Esquema de la CLI como JSON para introspección del agente |
| **`skill`** | Guía de habilidades para agentes para inyección de indicaciones |
| **`session`** | REPL JSON de entrada/salida estándar con 22 comandos |
| **Códigos de salida** | 0=éxito, 1=ejecución, 2=entrada, 3=tiempo de espera, 4=red |

### Motor JavaScript (ES2024+)

Impulsado por [`boa_engine`](https://boajs.dev/) — Rust puro, sin dependencia de V8:

| Web API | Estado |
|---------|--------|
| `document.querySelector` / `querySelectorAll` | ✅ Completa |
| `document.createElement` / `createTextNode` | ✅ Completa |
| `element.appendChild` / `removeChild` / `insertBefore` | ✅ Completa |
| `element.getAttribute` / `setAttribute` / `removeAttribute` | ✅ Completa |
| `element.cloneNode` / `remove()` | ✅ Completa |
| `element.style` (CSSStyleDeclaration) | ✅ Accesador de propiedad |
| `element.classList` (DOMTokenList) | ✅ Accesador de propiedad |
| `element.textContent` / `innerHTML` | ✅ Lectura/Escritura |
| `element.addEventListener` / `dispatchEvent` | ✅ Completa |
| `element.click()` | ✅ Con manejadores de eventos |
| `fetch()` | ✅ Completa (puente de canal) |
| `XMLHttpRequest` | ✅ Completa con devoluciones de llamada |
| `localStorage` | ✅ Persistente |
| `MutationObserver` | ✅ observe/disconnect/takeRecords |
| `setTimeout` / `setInterval` | ✅ TokioJobQueue |
| `console.log/warn/error` | ✅ Con formato |
| `URL` / `URLSearchParams` | ✅ Completa |
| `crypto.getRandomValues` | ✅ Pseudoaleatorio |
| `TextEncoder` / `TextDecoder` | ✅ UTF-8 |
| `atob` / `btoa` | ✅ Base64 |
| `requestAnimationFrame` | ✅ Polyfill |

### Protocolo CDP (Chrome DevTools Protocol)

10 manejadores de dominios — compatible con Puppeteer y Playwright:

| Dominio | Métodos clave |
|--------|------------|
| **Browser** | `getVersion`, `close` |
| **DOM** | `getDocument`, `describeNode`, `querySelector`, `querySelectorAll` |
| **Fetch** | `enable/disable`, `continueRequest`, `failRequest`, `fulfillRequest`, `getResponseBody` |
| **Input** | `dispatchKeyEvent`, `dispatchMouseEvent`, `insertText` |
| **Network** | `enable/disable`, `setExtraHTTPHeaders`, `getResponseBody` |
| **OXI** 🤖 | `getMarkdown`, `getPageInfo` — Extensiones nativas para IA |
| **Page** | `navigate`, `captureScreenshot`, `getFrameTree`, `getTitle` |
| **Runtime** | `evaluate`, `callFunctionOn`, `enable`, `consoleAPICalled` |
| **Target** | `getTargets`, `attachToTarget`, `detachFromTarget` |

### Dominio OXI — Diseñado para agentes de IA

```python
import websockets, json, asyncio

async def ai_scrape():
    ws = await websockets.connect('ws://localhost:9222/ws')
    
    await ws.send(json.dumps({
        "id": 1, "method": "Page.navigate",
        "params": {"url": "https://news.ycombinator.com"}
    }))
    await asyncio.sleep(2)
    
    # Markdown limpio — perfecto para ingestión de LLM
    await ws.send(json.dumps({"id": 2, "method": "OXI.getMarkdown"}))
    resp = json.loads(await ws.recv())
    print(resp['result']['markdown'])
```

### Capa de Red

| Característica | Descripción |
|---------|-------------|
| **Cliente HTTP** | `reqwest` con persistencia de cookies y seguimiento de redirecciones |
| **Tarjeta de cookies** | Almacenamiento de cookies con ámbito de dominio y análisis de `Set-Cookie` |
| **Protección SSRF** | Bloqueo CIDR para rangos de red privada |
| **robots.txt** | Analizador conforme a RFC 9309, marca `--obey-robots` |
| **Intercepción de red** | Pausar, modificar o bloquear cualquier solicitud a través del dominio Fetch |
| **Encabezados personalizados** | Inyección de encabezados por sesión y por solicitud |
| **Detección de charset** | `encoding_rs` para detección y conversión automática de conjuntos de caracteres |

### Renderizado de Texto CSS

- **Salida de texto ASCII/Unicode** — Renderizar DOM a texto legible con sangría adecuada
- **Conversión a Markdown** — HTML→Markdown completo con soporte para encabezados, enlaces y listas
- **Capturas PNG** — Fuente de mapa de bits 8×16 integrada, renderiza contenido de texto como imágenes
- **Sin dependencias externas** — Datos de fuente incrustados en el binario

---

## 🧪 Pruebas

```bash
# Ejecutar todas las pruebas
cargo test --workspace

# Pruebas de integración de la CLI (rápidas, sin red)
cargo test -p oxibrowser --test cli

# Pruebas CDP de extremo a extremo
cargo test -p oxibrowser-cdp

# Pruebas de integración (sitios web reales, requiere internet)
cargo test --workspace -- --ignored
```

---

## 🔧 Uso avanzado

### API de Rust

```rust
use oxibrowser_core::Browser;
use oxibrowser_core::config::BrowserConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let browser = Browser::new(BrowserConfig::default()).await?;
    let session = browser.new_session().await?;
    
    session.navigate("https://example.com").await?;
    
    let title = session.evaluate("document.title").await?;
    println!("Title: {:?}", title);
    
    Ok(())
)
}
```

### Uso como biblioteca

```toml
[dependencies]
oxibrowser-core = "0.11"
# O el servidor CDP:
oxibrowser-cdp = "0.11"
```

### Intercepción de solicitudes

```javascript
const client = await page.target().createCDPSession();

await client.send('Fetch.enable', {
    patterns: [{ urlPattern: '*ads*' }]
});

client.on('Fetch.requestPaused', async ({ requestId }) => {
    await client.send('Fetch.failRequest', {
        requestId,
        reason: 'BlockedByClient'
    });
});
```

---

## 🤝 Contribuir

Consulta [CONTRIBUTING.md](CONTRIBUTING.md) para las pautas completas.

```bash
git clone https://github.com/a7garden/oxibrowser.git
cd oxibrowser
cargo build
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

---

## 📄 Licencia

OxiBrowser está licenciado bajo la [Licencia MIT](LICENSE).

## 🙏 Agradecimientos

- [boa_engine](https://boajs.dev/) — Motor JavaScript en Rust puro (ES2024+)
- [html5ever](https://github.com/servo/html5ever) — Analizador HTML del proyecto Servo
- [reqwest](https://github.com/seanmonstar/reqwest) — Cliente HTTP ergonómico para Rust
- [tokio](https://tokio.rs/) — Runtime asíncrono que impulsa toda la pila de red

---

<div align="center">

**[⬆ Volver al inicio](#-oxibrowser)**

Hecho con 🦀 en Rust

</div>
