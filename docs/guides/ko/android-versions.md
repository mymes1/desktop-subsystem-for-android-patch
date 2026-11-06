# Android 버전

Desktop Subsystem for Android™(DSA)는 가장 새로운 Android 주 버전 두 개를 지원합니다. 지금은 Android
16과 Android 17입니다.

- 두 단계 더 새로운 주 버전이 나오면 지원이 끝납니다. Android 16은 Android 18이 나올 때까지 지원합니다.
- 이미지 업데이터는 지원하는 버전으로 옮겨 주며, 스스로 낮은 주 버전으로 옮기지는 않습니다.
- 이미지는 쓰기 전에 서명된 목록과 대조합니다. 부팅에 실패한 이미지는 이전 이미지로 되돌립니다.

## 버전 고르기

DSA 설정의 **Android** 페이지나 다음 명령을 씁니다.

```
dsa android version          # 실행 중인 버전과 고를 수 있는 버전
dsa android use 17           # 주 버전 고르기
dsa android use latest
```

더 오래된 주 버전으로 옮기면 Android 데이터(설치한 앱, 계정, 앱 데이터)가 지워집니다. Android는 더 새로운
버전이 쓴 데이터를 읽지 못하기 때문입니다. 그래서 이 경우 명령에 `--erase-data`가 있어야 진행합니다.

이미지 업데이트를 끄려면 관리자가 `dsa settings set image-updates off`를 실행합니다.

---

Android는 Google LLC의 상표입니다. DSA는 Google과 제휴하거나 Google의 보증 또는 후원을 받지 않습니다.
