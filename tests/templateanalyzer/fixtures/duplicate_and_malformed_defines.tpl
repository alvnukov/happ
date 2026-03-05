{{- define "dup.a" -}}first{{- end -}}
{{- define "dup.a" -}}second{{- end -}}
{{- define bad.name -}}BAD{{- end -}}
{{- define 'single.q' -}}{{ $.Values.x_y-z.v1 }}{{- end -}}
