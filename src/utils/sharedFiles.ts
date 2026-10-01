import { invoke } from '@tauri-apps/api/core';
import type { FileTextResult } from '../hooks/useTauriCommands';

export interface SharedFile {
  path: string;
  filename: string;
  size: number;
}

export function saveSharedFile(sourcePath: string): Promise<SharedFile> {
  return invoke('save_shared_file', { sourcePath });
}

export async function prepareSharedFile(sourcePath: string) {
  const file = await saveSharedFile(sourcePath);
  let result: FileTextResult;
  try {
    result = await invoke<FileTextResult>('extract_file_text', { sourcePath: file.path });
  } catch {
    // A file can still be shared when its text cannot be extracted (e.g. a scanned PDF).
    result = { filename: file.filename, text: '', file_type: 'unsupported', truncated: false, original_char_count: 0 };
  }
  return { file, result };
}

export function fileMessage(result: FileTextResult): string {
  const text = result.file_type === 'unsupported'
    ? '已分享文件。此格式暂不支持内容提取，请勿推测文件内容。'
    : result.text || '未提取到可阅读的文本，请勿推测文件内容。';
  const hint = result.truncated ? `\n（文件过长，已截断，原始 ${result.original_char_count} 字符）` : '';
  return `[文件：${result.filename}]\n${text}${hint}`;
}

export function fileMetadata(result: FileTextResult, file: SharedFile) {
  return {
    kind: 'file', file_name: file.filename, file_type: result.file_type,
    file_path: file.path, file_size: file.size,
    truncated: result.truncated, original_char_count: result.original_char_count,
  };
}
