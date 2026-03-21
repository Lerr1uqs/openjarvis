import * as Lark from '@larksuiteoapi/node-sdk';

const appId = process.env.FEISHU_APP_ID ?? '';
const appSecret = process.env.FEISHU_APP_SECRET ?? '';
const openBaseUrl = process.env.FEISHU_OPEN_BASE_URL ?? 'https://open.feishu.cn';

if (!appId || !appSecret) {
  console.error('[feishu-ws] missing FEISHU_APP_ID / FEISHU_APP_SECRET');
  process.exit(1);
}

const domain = openBaseUrl.replace(/\/+$/, '');
const logger = {
  debug: (...args) => console.error(...args),
  info: (...args) => console.error(...args),
  warn: (...args) => console.error(...args),
  error: (...args) => console.error(...args),
};

const baseConfig = {
  appId,
  appSecret,
  domain,
};

const wsClient = new Lark.WSClient({
  ...baseConfig,
  loggerLevel: Lark.LoggerLevel.debug,
  logger,
});

const eventDispatcher = new Lark.EventDispatcher({}).register({
  'im.message.receive_v1': async (data) => {
    const payload = {
      event_id: data.header?.event_id ?? null,
      sender_open_id: data.sender?.sender_id?.open_id ?? '',
      sender_type: data.sender?.sender_type ?? '',
      tenant_key: data.sender?.tenant_key ?? '',
      message_id: data.message?.message_id ?? '',
      chat_id: data.message?.chat_id ?? '',
      thread_id: data.message?.thread_id ?? null,
      chat_type: data.message?.chat_type ?? '',
      message_type: data.message?.message_type ?? '',
      content: data.message?.content ?? '',
    };

    process.stdout.write(`${JSON.stringify(payload)}\n`);
    return 'ok';
  },
});

process.on('SIGINT', () => process.exit(0));
process.on('SIGTERM', () => process.exit(0));

console.error('[feishu-ws] starting long connection');
wsClient.start({ eventDispatcher });
