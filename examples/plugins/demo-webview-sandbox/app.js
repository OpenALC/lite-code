(function () {
  'use strict';
  var log = document.getElementById('log');
  function line(text, cls) {
    var d = document.createElement('div');
    d.textContent = text;
    if (cls) d.className = cls;
    log.appendChild(d);
  }
  line('script-loaded', 'ok');
  var nonce = decodeURIComponent((location.hash || '').replace(/^#/, ''));
  if (!nonce) { line('no-nonce', 'err'); return; }
  line('nonce-received', 'ok');
  var seq = 0;
  var pending = new Map();
  window.addEventListener('message', function (event) {
    var msg = event.data;
    if (!msg || typeof msg !== 'object') return;
    if (msg.op === 'init' && event.ports && event.ports[0]) {
      var port = event.ports[0];
      window.__port = port;
      port.onmessage = function (e) {
        var m = e.data;
        if (m && typeof m.seq === 'number' && pending.has(m.seq)) {
          pending.get(m.seq)(m);
          pending.delete(m.seq);
        }
      };
      line('port-ready', 'ok');
      // 三个 bridge op 各走一遍：getState → setState → postCommand。
      call('getState', { key: 'greet' }, function (r) {
        line('getState=' + JSON.stringify(r.ok ? r.value : r.error), r.ok ? 'ok' : 'err');
        call('setState', { key: 'greet', value: 'hello-from-sandbox' }, function (r2) {
          line('setState=' + (r2.ok ? 'ok' : JSON.stringify(r2.error)), r2.ok ? 'ok' : 'err');
          call('getState', { key: 'greet' }, function (r3) {
            line('getState2=' + JSON.stringify(r3.ok ? r3.value : r3.error), r3.ok ? 'ok' : 'err');
            call('postCommand', { command: 'demo.webview.ping', args: { from: 'sandbox' } }, function (r4) {
              line('postCommand=' + JSON.stringify(r4.ok ? r4.result : r4.error), r4.ok ? 'ok' : 'err');
              call('postCommand', { command: 'demo.ui-live.open', args: {} }, function (r5) {
                line('cross-plugin-rejected=' + (!r5.ok), !r5.ok ? 'ok' : 'err');
              });
            });
          });
        });
      });
    }
  });
  function call(op, params, cb) {
    var s = ++seq;
    pending.set(s, cb);
    window.__port.postMessage(Object.assign({ source: 'lc-plugin', protocol: 1, op: op, seq: s }, params));
  }
  window.parent.postMessage({ source: 'lc-plugin', protocol: 1, op: 'ready', nonce: nonce }, '*');
  line('ready-sent', 'ok');
})();
