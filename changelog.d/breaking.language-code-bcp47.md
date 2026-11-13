- **`LanguageCode::parse` stores BCP 47 display casing** (`adk-realtime`): a
  2–3 letter language and later subtags of 2–8 letters or digits are kept as
  `en-US` and `zh-Hant`. A longer primary subtag or a one-character subtag is
  rejected.
