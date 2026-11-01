!macro ExecChecked Command Success Failure
    ClearErrors
    ExecWait '${Command}' $0
    IfErrors ${Failure}
    IntCmp $0 0 ${Success} ${Failure} ${Failure}
!macroend

!macro DeleteChecked Path Failure
    ClearErrors
    Delete "${Path}"
    IfErrors ${Failure}
!macroend
