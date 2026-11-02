UPDATE hpa_control.members
SET base_role = role
WHERE base_role = 'viewer' AND role <> 'viewer';
