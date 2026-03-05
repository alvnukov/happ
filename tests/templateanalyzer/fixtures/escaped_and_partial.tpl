values:
  keep: '{{ include "safe.a" . }}'
  escaped: '{{ "{{" }} include "ignored.literal" . {{ "}}" }}'
  broken_tail: '{{ include "half.open" . '

{{- define "safe.a" -}}
{{ .Values.good.path }}-{{ .Values.bad..path }}
{{- end -}}
