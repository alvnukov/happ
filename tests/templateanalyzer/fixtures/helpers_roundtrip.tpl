{{- define "foo.name" -}}
foo
{{- end -}}

{{- define "foo.cluster-name" -}}
{{- default (include "foo.name" .) .Values.cluster.name -}}
{{- end -}}

kind: ConfigMap
metadata:
  name: '{{ include "foo.cluster-name" . }}'
data:
  sa: '{{ $.Values.serviceAccount.name }}'
