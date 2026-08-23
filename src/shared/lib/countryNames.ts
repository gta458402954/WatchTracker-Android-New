export type CountryCode = string;
export type RegionFilter = 'all' | CountryCode;

export const UNKNOWN_REGION_CODE = '__UNKNOWN__';
export const UNKNOWN_REGION_LABEL = '未知地区';

export const PREFERRED_COUNTRY_CODES = [
  'CN',
  'HK',
  'TW',
  'US',
  'JP',
  'KR',
  'GB',
] as const;

export const ISO_COUNTRY_NAMES: Readonly<Record<string, string>> = {
  CN: '中国大陆',
  HK: '中国香港',
  TW: '中国台湾',
  JP: '日本',
  KR: '韩国',
  IN: '印度',
  TH: '泰国',
  ID: '印度尼西亚',
  MY: '马来西亚',
  SG: '新加坡',
  VN: '越南',
  PH: '菲律宾',
  IR: '伊朗',
  TR: '土耳其',
  IL: '以色列',
  SA: '沙特阿拉伯',
  AE: '阿联酋',
  KZ: '哈萨克斯坦',
  US: '美国',
  CA: '加拿大',
  MX: '墨西哥',
  GB: '英国',
  FR: '法国',
  DE: '德国',
  IT: '意大利',
  ES: '西班牙',
  RU: '俄罗斯',
  SE: '瑞典',
  NO: '挪威',
  DK: '丹麦',
  FI: '芬兰',
  NL: '荷兰',
  BE: '比利时',
  CH: '瑞士',
  AT: '奥地利',
  PL: '波兰',
  CZ: '捷克',
  HU: '匈牙利',
  GR: '希腊',
  PT: '葡萄牙',
  IE: '爱尔兰',
  IS: '冰岛',
  UA: '乌克兰',
  RO: '罗马尼亚',
  BG: '保加利亚',
  BY: '白俄罗斯',
  HR: '克罗地亚',
  RS: '塞尔维亚',
  BR: '巴西',
  AR: '阿根廷',
  CL: '智利',
  CO: '哥伦比亚',
  PE: '秘鲁',
  VE: '委内瑞拉',
  AU: '澳大利亚',
  NZ: '新西兰',
  ZA: '南非',
  EG: '埃及',
  NG: '尼日利亚',
  KE: '肯尼亚',
  MA: '摩洛哥',
  DZ: '阿尔及利亚',
  CU: '古巴',
  JM: '牙买加',
  DO: '多米尼加',
};

const COUNTRY_CODE_BY_LABEL: Readonly<Record<string, CountryCode>> = Object.freeze(
  Object.fromEntries(Object.entries(ISO_COUNTRY_NAMES).map(([code, label]) => [label, code])),
);

const COUNTRY_ALIASES: Readonly<Record<string, CountryCode>> = {
  UK: 'GB',
  中国: 'CN',
  大陆: 'CN',
  内地: 'CN',
  香港: 'HK',
  台湾: 'TW',
  南韩: 'KR',
  俄国: 'RU',
  大不列颠: 'GB',
};

export function countryLabelOf(code: CountryCode): string {
  if (code === UNKNOWN_REGION_CODE) return UNKNOWN_REGION_LABEL;
  const normalizedCode = code.trim().toUpperCase();
  return ISO_COUNTRY_NAMES[normalizedCode] ?? normalizedCode;
}

export function countryCodeOfLabel(label: string): CountryCode | undefined {
  const normalizedLabel = label.trim();
  if (!normalizedLabel) return undefined;
  return COUNTRY_CODE_BY_LABEL[normalizedLabel] ?? COUNTRY_ALIASES[normalizedLabel.toUpperCase()];
}

export function isCountryLabel(label: string): boolean {
  return countryCodeOfLabel(label) !== undefined;
}
