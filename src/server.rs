//! HTTP surface: a small single-page web UI + JSON API over the debate and
//! validate engines. POST /api/debate and /api/validate; GET / serves the page.

use crate::config::{parse_protocol, Config};
use axum::{
    extract::State,
    response::Html,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Clone)]
struct AppState {
    cfg: Arc<Config>,
}

#[derive(Deserialize)]
struct DebateReq {
    prompt: String,
    #[serde(default)]
    rounds: Option<u32>,
    #[serde(default)]
    protocol: Option<String>,
}

#[derive(Deserialize)]
struct ValidateReq {
    statement: String,
    #[serde(default)]
    reviewer: Option<String>,
    #[serde(default)]
    context: Option<String>,
}

pub async fn serve(config_path: Option<String>, host: &str, port: u16) -> anyhow::Result<()> {
    let cfg = Config::load_default(config_path.as_deref())?;
    let state = AppState {
        cfg: Arc::new(cfg),
    };
    let app = Router::new()
        .route("/", get(index))
        .route("/api/debate", post(debate_handler))
        .route("/api/validate", post(validate_handler))
        .with_state(state);
    let addr = parse_bind_addr(host, port)?;
    if !addr.ip().is_loopback() {
        eprintln!(
            "warning: binding to {} exposes the UNAUTHENTICATED web UI on non-local interfaces",
            addr.ip()
        );
    }
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("Abe web UI: http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Parse a host string + port into a bind address. Host must be a literal IP
/// (e.g. 127.0.0.1 or 0.0.0.0) — hostnames are not resolved here.
fn parse_bind_addr(host: &str, port: u16) -> anyhow::Result<std::net::SocketAddr> {
    let ip: std::net::IpAddr = host
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid bind host {host:?} (expected an IP like 127.0.0.1)"))?;
    Ok(std::net::SocketAddr::new(ip, port))
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

fn err_json(e: impl std::fmt::Display) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "error": e.to_string() }))
}

async fn debate_handler(
    State(s): State<AppState>,
    Json(req): Json<DebateReq>,
) -> Json<serde_json::Value> {
    let mut cfg = (*s.cfg).clone();
    if let Some(r) = req.rounds {
        cfg.debate.rounds = r;
    }
    if let Some(p) = &req.protocol {
        match parse_protocol(p) {
            Ok(pr) => cfg.debate.protocol = pr,
            Err(e) => return err_json(e),
        }
    }
    match crate::debate::debate_from_config(&cfg, &req.prompt).await {
        Ok(res) => Json(serde_json::to_value(res).unwrap_or_else(|e| {
            serde_json::json!({ "error": e.to_string() })
        })),
        Err(e) => err_json(e),
    }
}

async fn validate_handler(
    State(s): State<AppState>,
    Json(req): Json<ValidateReq>,
) -> Json<serde_json::Value> {
    match crate::validate::validate_from_config(
        &s.cfg,
        &req.statement,
        req.reviewer.as_deref(),
        req.context.as_deref(),
    )
    .await
    {
        Ok(res) => Json(serde_json::to_value(res).unwrap_or_else(|e| {
            serde_json::json!({ "error": e.to_string() })
        })),
        Err(e) => err_json(e),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_bind_addr;

    #[test]
    fn parses_valid_hosts_and_rejects_garbage() {
        assert_eq!(parse_bind_addr("127.0.0.1", 8080).unwrap().to_string(), "127.0.0.1:8080");
        assert!(parse_bind_addr("0.0.0.0", 80).unwrap().ip().is_unspecified());
        assert!(parse_bind_addr("not-an-ip", 8080).is_err());
    }
}

const INDEX_HTML: &str = r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Abe — multi-model debate</title>
<style>
body{font-family:system-ui,sans-serif;max-width:820px;margin:2rem auto;padding:0 1rem;color:#1a1a1a;background:#fafafa}
h1{font-size:1.4rem;margin-bottom:.2rem}
.sub{color:#666;font-size:.85rem;margin-bottom:1rem}
textarea{width:100%;min-height:90px;font:inherit;padding:.6rem;box-sizing:border-box;border:1px solid #ccc;border-radius:6px}
.row{display:flex;gap:.7rem;align-items:center;margin:.6rem 0;flex-wrap:wrap}
button{font:inherit;padding:.5rem 1rem;cursor:pointer;border:1px solid #888;border-radius:6px;background:#fff}
button.primary{background:#2563eb;color:#fff;border-color:#2563eb}
button:disabled{opacity:.5;cursor:default}
select,input[type=number]{font:inherit;padding:.35rem}
.card{background:#fff;border:1px solid #ddd;border-radius:8px;padding:1rem;margin-top:1rem}
.final{font-size:1.05rem;white-space:pre-wrap}
ul.agree li{color:#15803d}
ul.disagree li{color:#b45309}
.muted{color:#666;font-size:.85rem}
pre{white-space:pre-wrap;word-break:break-word;margin:0}
h3{margin:.8rem 0 .3rem}
</style></head>
<body>
<h1>Abe</h1>
<div class="sub">multi-model debate &amp; second-opinion validation &mdash; named for Lincoln</div>
<div class="row">
  <label><input type="radio" name="mode" value="debate" checked> Debate</label>
  <label><input type="radio" name="mode" value="validate"> Validate</label>
</div>
<textarea id="input" placeholder="Ask a question to debate, or state a decision to validate..."></textarea>
<div class="row" id="debateOpts">
  <label>Rounds <input id="rounds" type="number" min="0" placeholder="cfg" style="width:4.5rem"></label>
  <label>Protocol
    <select id="protocol">
      <option value="">(config)</option>
      <option>synthesis</option><option>majority</option><option>judge</option>
    </select>
  </label>
</div>
<div class="row">
  <button class="primary" id="run">Run</button>
  <span class="muted" id="status"></span>
</div>
<div id="out"></div>
<script>
const $=s=>document.querySelector(s);
const mode=()=>document.querySelector('input[name=mode]:checked').value;
const esc=s=>(s||'').replace(/[&<>]/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;'}[c]));
function toggle(){$('#debateOpts').style.display=mode()==='debate'?'flex':'none';}
document.querySelectorAll('input[name=mode]').forEach(r=>r.addEventListener('change',toggle));toggle();
async function run(){
  const text=$('#input').value.trim();if(!text)return;
  $('#status').textContent='running (CLI models can take ~10-60s)...';$('#run').disabled=true;$('#out').innerHTML='';
  try{
    if(mode()==='debate'){
      const body={prompt:text};
      const r=$('#rounds').value;if(r!=='')body.rounds=parseInt(r,10);
      const p=$('#protocol').value;if(p)body.protocol=p;
      renderDebate(await post('/api/debate',body));
    }else{
      renderValidate(await post('/api/validate',{statement:text}));
    }
  }catch(e){$('#out').innerHTML='<div class="card">error: '+esc(e.message)+'</div>';}
  $('#status').textContent='';$('#run').disabled=false;
}
async function post(url,body){
  const res=await fetch(url,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify(body)});
  return res.json();
}
function renderDebate(r){
  if(r.error){$('#out').innerHTML='<div class="card">error: '+esc(r.error)+'</div>';return;}
  let h='<div class="card"><div class="muted">'+esc(r.protocol)+' &middot; '+esc((r.models_used||[]).join(', '))+'</div>';
  h+='<p class="final">'+esc(r.final_answer)+'</p>';
  const ag=(r.report&&r.report.agreements)||[],dis=(r.report&&r.report.disagreements)||[];
  if(ag.length)h+='<h3>Agreements</h3><ul class="agree">'+ag.map(a=>'<li>'+esc(a)+'</li>').join('')+'</ul>';
  if(dis.length)h+='<h3>Disagreements</h3><ul class="disagree">'+dis.map(a=>'<li>'+esc(a)+'</li>').join('')+'</ul>';
  h+='</div>';$('#out').innerHTML=h;
}
function renderValidate(r){
  if(r.error){$('#out').innerHTML='<div class="card">error: '+esc(r.error)+'</div>';return;}
  $('#out').innerHTML='<div class="card"><div class="muted">reviewer: '+esc(r.reviewer)+'</div><pre>'+esc(r.take)+'</pre></div>';
}
$('#run').addEventListener('click',run);
</script>
</body></html>
"##;
