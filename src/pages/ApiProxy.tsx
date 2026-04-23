import { useEffect } from 'react';
import { useNavigate } from 'react-router-dom';

/**
 * API 反代 — 已拆分为"服务配置"和"路由管理"两个独立页面。
 * 保留此路由以兼容旧书签/导航，自动跳转到服务配置页。
 */
export default function ApiProxy() {
    const navigate = useNavigate();

    useEffect(() => {
        navigate('/service-config', { replace: true });
    }, [navigate]);

    return (
        <div className="flex items-center justify-center min-h-[60vh]">
            <span className="loading loading-spinner loading-lg text-blue-500"></span>
        </div>
    );
}
