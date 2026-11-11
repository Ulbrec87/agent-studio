import type { editor } from 'monaco-editor'

export const hydraLight: editor.IStandaloneThemeData = {
  base: 'vs',
  inherit: true,
  rules: [
    { token: 'comment', foreground: '7A6F66', fontStyle: 'italic' },
    { token: 'keyword', foreground: 'C4623F' },
    { token: 'string', foreground: '4CAF50' },
    { token: 'number', foreground: 'D97757' },
    { token: 'type', foreground: '8B5E3C' }
  ],
  colors: {
    'editor.background': '#FAF6F1',
    'editor.foreground': '#2D2420',
    'editor.lineHighlightBackground': '#F5F0EA',
    'editor.selectionBackground': '#E5DFD8',
    'editorLineNumber.foreground': '#B0A79E',
    'editorLineNumber.activeForeground': '#7A6F66',
    'editorCursor.foreground': '#D97757',
    'editor.inactiveSelectionBackground': '#EDE8E2',
    'editorIndentGuide.background': '#E5DFD8',
    'editorIndentGuide.activeBackground': '#B0A79E',
    'editorWidget.background': '#FFFFFF',
    'editorWidget.border': '#E5DFD8'
  }
}

export const hydraDark: editor.IStandaloneThemeData = {
  base: 'vs-dark',
  inherit: true,
  rules: [
    { token: 'comment', foreground: '6B6760', fontStyle: 'italic' },
    { token: 'keyword', foreground: 'E08B6D' },
    { token: 'string', foreground: '66BB6A' },
    { token: 'number', foreground: 'FFA726' },
    { token: 'type', foreground: 'E08B6D' }
  ],
  colors: {
    'editor.background': '#262624',
    'editor.foreground': '#E8E4E0',
    'editor.lineHighlightBackground': '#30302e',
    'editor.selectionBackground': '#3b3b38',
    'editorLineNumber.foreground': '#6B6760',
    'editorLineNumber.activeForeground': '#A8A49E',
    'editorCursor.foreground': '#E08B6D',
    'editor.inactiveSelectionBackground': '#353532',
    'editorIndentGuide.background': '#3b3b38',
    'editorIndentGuide.activeBackground': '#6B6760',
    'editorWidget.background': '#30302e',
    'editorWidget.border': '#3b3b38'
  }
}

export const hydraMidnight: editor.IStandaloneThemeData = {
  base: 'vs-dark',
  inherit: true,
  rules: [
    { token: 'comment', foreground: '666666', fontStyle: 'italic' },
    { token: 'keyword', foreground: 'D4D4D4' },
    { token: 'string', foreground: '7EE787' },
    { token: 'number', foreground: 'FFD166' },
    { token: 'type', foreground: 'A0A0A0' }
  ],
  colors: {
    'editor.background': '#191919',
    'editor.foreground': '#E8E8E8',
    'editor.lineHighlightBackground': '#232323',
    'editor.selectionBackground': '#333333',
    'editorLineNumber.foreground': '#666666',
    'editorLineNumber.activeForeground': '#A0A0A0',
    'editorCursor.foreground': '#E8E8E8',
    'editor.inactiveSelectionBackground': '#2a2a2a',
    'editorIndentGuide.background': '#333333',
    'editorIndentGuide.activeBackground': '#666666',
    'editorWidget.background': '#232323',
    'editorWidget.border': '#333333'
  }
}

export const workspaceSoft: editor.IStandaloneThemeData = {
  base: 'vs-dark',
  inherit: true,
  rules: [
    { token: 'comment', foreground: 'A5AFBD', fontStyle: 'italic' },
    { token: 'keyword', foreground: 'C3A5D8' },
    { token: 'string', foreground: 'D6B08B' },
    { token: 'number', foreground: 'A6C6A3' },
    { token: 'type', foreground: '92BECE' }
  ],
  colors: {
    'editor.background': '#12171D',
    'editor.foreground': '#E2E6EC',
    'editor.lineHighlightBackground': '#232C36',
    'editor.selectionBackground': '#36495E',
    'editor.inactiveSelectionBackground': '#2E3B49',
    'editorLineNumber.foreground': '#A5AFBD',
    'editorLineNumber.activeForeground': '#E2E6EC',
    'editorCursor.foreground': '#B2C8E1',
    'editorIndentGuide.background1': '#2E3741',
    'editorIndentGuide.activeBackground1': '#5B6B7D',
    'editorWidget.background': '#242C35',
    'editorWidget.border': '#46515F',
    'editorGutter.background': '#12171D'
  }
}

export const THEME_MAP: Record<string, { name: string; data: editor.IStandaloneThemeData }> = {
  'modern-workspace-soft': { name: 'workspace-soft', data: workspaceSoft },
  light: { name: 'hydra-light', data: hydraLight },
  dark: { name: 'hydra-dark', data: hydraDark },
  midnight: { name: 'hydra-midnight', data: hydraMidnight }
}
